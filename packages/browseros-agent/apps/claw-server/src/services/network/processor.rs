//! QuickJS sandbox for response-stage scripts.
//!
//! Every call gets a fresh runtime: no state survives between requests, and the
//! only host capability is a lazily-read upstream cookie jar.

use browseros_core::network::{
    HeaderPair, OneOrMany, ProcessorError, ProcessorErrorKind, ProcessorHooks, ProcessorInput,
    ProcessorResult, ProcessorRun, ProcessorSpec, ResponseProcessor, base64_decode, base64_encode,
    parse_url_parts,
};
use futures_util::future::BoxFuture;
use rquickjs::{Context, Ctx, Function, Runtime};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

const MAX_CONCURRENT: usize = 4;
const PERMIT_WAIT: Duration = Duration::from_secs(1);
const MEMORY_LIMIT: usize = 32 * 1024 * 1024;
const STACK_LIMIT: usize = 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const SCRIPT_LIMIT_BYTES: usize = 64 * 1024;
const VALIDATE_TIMEOUT: Duration = Duration::from_millis(200);

const PRELUDE: &str = r#""use strict";
const __neob_logs = [];
const __neob_fmt = (v) => {
  if (typeof v === 'string') return v;
  try { return JSON.stringify(v); } catch (_) { return String(v); }
};
const __neob_log = (...args) => {
  if (__neob_logs.length < 50) __neob_logs.push(args.map(__neob_fmt).join(' ').slice(0, 500));
};
globalThis.console = Object.freeze({ log: __neob_log, warn: __neob_log, error: __neob_log });
const __neob_in = JSON.parse(globalThis.__neob_input_json);
delete globalThis.__neob_input_json;
const __neob_list = (pairs) => Object.freeze(pairs.map((p) => Object.freeze([p[0], p[1]])));
const __neob_get = (list) => (name) => {
  const key = String(name).toLowerCase();
  const hit = list.find((p) => p[0] === key);
  return hit ? hit[1] : null;
};
const __neob_all = (list) => (name) => {
  const key = String(name).toLowerCase();
  return list.filter((p) => p[0] === key).map((p) => p[1]);
};
// Registered by the engine before the script runs; [] when the host cannot
// read the upstream cookies (see §2.6).
const __neob_read_cookies =
  typeof globalThis.__neob_upstream_cookies === 'function'
    ? () => JSON.parse(globalThis.__neob_upstream_cookies())
    : () => [];
const __neob_req_headers = __neob_list(__neob_in.request.headers);
const __neob_res_headers = __neob_list(__neob_in.response.headers);
const __neob_ctx = Object.freeze({
  request: Object.freeze({
    method: __neob_in.request.method,
    url: __neob_in.request.url,
    origin: __neob_in.request.origin,
    pathname: __neob_in.request.pathname,
    query: Object.freeze(Object.fromEntries(
      Object.entries(__neob_in.request.query).map(([k, v]) => [k, Object.freeze(v.slice())]))),
    headers: __neob_req_headers,
    body: __neob_in.request.body,
    header: __neob_get(__neob_req_headers),
    headerAll: __neob_all(__neob_req_headers),
    cookies() {
      const out = {};
      for (const line of __neob_all(__neob_req_headers)('cookie')) {
        for (const part of line.split(';')) {
          const i = part.indexOf('=');
          if (i <= 0) continue;
          const name = part.slice(0, i).trim();
          if (name && !(name in out)) out[name] = part.slice(i + 1).trim();
        }
      }
      return out;
    },
  }),
  response: Object.freeze({
    status: __neob_in.response.status,
    statusText: __neob_in.response.statusText,
    headers: __neob_res_headers,
    encoding: __neob_in.response.encoding,
    body: __neob_in.response.body,
    bodyBase64: __neob_in.response.bodyBase64,
    json() {
      if (__neob_in.response.body === null) throw new TypeError('response body is binary; use bodyBase64');
      return JSON.parse(__neob_in.response.body);
    },
    header: __neob_get(__neob_res_headers),
    headerAll: __neob_all(__neob_res_headers),
    cookies: __neob_read_cookies,
    setCookies() {
      return __neob_all(__neob_res_headers)('set-cookie').map((raw) => {
        const first = raw.split(';')[0];
        const i = first.indexOf('=');
        return i < 0
          ? { name: '', value: first.trim(), raw }
          : { name: first.slice(0, i).trim(), value: first.slice(i + 1).trim(), raw };
      });
    },
  }),
});"#;

const EPILOGUE: &str = r#"(function () {
  if (typeof process !== 'function') throw new TypeError('script must define function process(ctx)');
  const out = process(__neob_ctx);
  if (out === undefined || out === null) return JSON.stringify({ passthrough: true });
  if (typeof out !== 'object' || Array.isArray(out)) throw new TypeError('process(ctx) must return an object or undefined');
  if (typeof out.then === 'function') throw new TypeError('process(ctx) must be synchronous');
  const allowed = ['status', 'statusText', 'headers', 'setHeaders', 'body', 'bodyBase64'];
  for (const key of Object.keys(out)) {
    if (!allowed.includes(key)) throw new TypeError('unknown return key: ' + key);
  }
  if (out.body !== undefined && out.bodyBase64 !== undefined) throw new TypeError('return either body or bodyBase64');
  const result = {};
  if (out.status !== undefined) result.status = out.status;
  if (out.statusText !== undefined) {
    if (typeof out.statusText !== 'string') throw new TypeError('statusText must be a string');
    result.statusText = out.statusText;
  }
  if (out.headers !== undefined) {
    if (!Array.isArray(out.headers)) throw new TypeError('headers must be an array of [name, value]');
    result.headers = out.headers.map((pair) => {
      if (!Array.isArray(pair) || pair.length !== 2) throw new TypeError('headers must be an array of [name, value]');
      return [String(pair[0]), String(pair[1])];
    });
  }
  if (out.setHeaders !== undefined) {
    if (typeof out.setHeaders !== 'object' || out.setHeaders === null || Array.isArray(out.setHeaders)) {
      throw new TypeError('setHeaders must be an object');
    }
    const patch = {};
    for (const [name, value] of Object.entries(out.setHeaders)) {
      if (value === null) patch[name] = null;
      else if (Array.isArray(value)) patch[name] = value.map(String);
      else patch[name] = String(value);
    }
    result.setHeaders = patch;
  }
  if (out.body !== undefined) {
    const text = typeof out.body === 'string' ? out.body : JSON.stringify(out.body);
    if (typeof text !== 'string') throw new TypeError('body must be a string or JSON-serializable value');
    result.bodyText = text;
  }
  if (out.bodyBase64 !== undefined) {
    if (typeof out.bodyBase64 !== 'string') throw new TypeError('bodyBase64 must be a string');
    result.bodyBase64 = out.bodyBase64;
  }
  return JSON.stringify(result);
})()"#;

/// Runs `process(ctx)` in QuickJS, at most [`MAX_CONCURRENT`] at a time.
pub struct ScriptProcessor {
    permits: Arc<Semaphore>,
}

impl Default for ScriptProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptProcessor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT)),
        }
    }
}

/// Rejects scripts that do not compile or never declare `process`.
pub fn validate_script(script: &str) -> Result<(), String> {
    if script.trim().is_empty() {
        return Err("script must be 1 byte - 64KiB".to_string());
    }
    if script.len() > SCRIPT_LIMIT_BYTES {
        return Err("script must be 1 byte - 64KiB".to_string());
    }
    let runtime = Runtime::new().map_err(|error| error.to_string())?;
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_max_stack_size(STACK_LIMIT);
    let deadline = Instant::now() + VALIDATE_TIMEOUT;
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context = Context::full(&runtime).map_err(|error| error.to_string())?;
    context.with(|ctx| {
        ctx.eval::<(), _>(script)
            .map_err(|error| format_js_error(&ctx, error))?;
        let declared: bool = ctx
            .eval("typeof process === 'function'")
            .map_err(|error| format_js_error(&ctx, error))?;
        if declared {
            Ok(())
        } else {
            Err("script must define function process(ctx)".to_string())
        }
    })
}

impl ResponseProcessor for ScriptProcessor {
    fn run<'a>(
        &'a self,
        spec: &'a ProcessorSpec,
        input: ProcessorInput,
        hooks: ProcessorHooks,
    ) -> BoxFuture<'a, Result<ProcessorRun, ProcessorError>> {
        Box::pin(async move {
            let permit =
                tokio::time::timeout(PERMIT_WAIT, Arc::clone(&self.permits).acquire_owned())
                    .await
                    .map_err(|_| processor_error(ProcessorErrorKind::Busy, "processor is busy"))?
                    .map_err(|_| {
                        processor_error(ProcessorErrorKind::Engine, "processor is shutting down")
                    })?;

            let started = Instant::now();
            let timeout = Duration::from_millis(u64::from(spec.timeout_ms));
            let payload = script_input(&input);
            let script = spec.script.clone();
            let hook: Arc<dyn Fn() -> Result<String, String> + Send + Sync> =
                Arc::from(hooks.upstream_cookies);
            let outcome = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let cache = Arc::new(Mutex::new(None::<String>));
                execute(&script, payload, timeout, hook, cache)
            })
            .await
            .map_err(|error| processor_error(ProcessorErrorKind::Engine, error.to_string()))?;

            let elapsed_ms = started.elapsed().as_millis() as u64;
            outcome.map(|(result, logs)| ProcessorRun {
                result,
                logs,
                elapsed_ms,
            })
        })
    }
}

fn processor_error(kind: ProcessorErrorKind, message: impl Into<String>) -> ProcessorError {
    ProcessorError {
        kind,
        message: message.into(),
        logs: Vec::new(),
    }
}

fn script_input(input: &ProcessorInput) -> String {
    let parts = parse_url_parts(&input.url);
    let mut query: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in &parts.query {
        query.entry(name.clone()).or_default().push(value.clone());
    }
    let (body, body_base64) = match std::str::from_utf8(&input.body) {
        Ok(text) => (Some(text.to_string()), None),
        Err(_) => (None, Some(base64_encode(&input.body))),
    };
    let encoding = input
        .response_headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-encoding"))
        .map(|header| header.value.clone());
    serde_json::json!({
        "request": {
            "method": input.method,
            "url": input.url,
            "origin": parts.origin,
            "pathname": parts.pathname,
            "query": query,
            "headers": lower_headers(&input.request_headers),
            "body": input.request_body,
        },
        "response": {
            "status": input.status,
            "statusText": input.status_text,
            "headers": lower_headers(&input.response_headers),
            "encoding": encoding,
            "body": body,
            "bodyBase64": body_base64,
        },
    })
    .to_string()
}

fn lower_headers(headers: &[HeaderPair]) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|header| (header.name.to_ascii_lowercase(), header.value.clone()))
        .collect()
}

type Outcome = (ProcessorResult, Vec<String>);

fn execute(
    script: &str,
    input_json: String,
    timeout: Duration,
    hook: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
    cache: Arc<Mutex<Option<String>>>,
) -> Result<Outcome, ProcessorError> {
    let runtime = Runtime::new()
        .map_err(|error| processor_error(ProcessorErrorKind::Engine, error.to_string()))?;
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_max_stack_size(STACK_LIMIT);
    let deadline = Instant::now() + timeout;
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context = Context::full(&runtime)
        .map_err(|error| processor_error(ProcessorErrorKind::Engine, error.to_string()))?;

    context.with(|ctx| {
        let cookies = Arc::clone(&cache);
        let hook = Arc::clone(&hook);
        let read_cookies = Function::new(ctx.clone(), move || -> String {
            let mut cached = cookies
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(value) = cached.as_ref() {
                return value.clone();
            }
            let value = hook().unwrap_or_else(|_| "[]".to_string());
            *cached = Some(value.clone());
            value
        })
        .map_err(|error| engine_error(&ctx, error))?;
        ctx.globals()
            .set("__neob_upstream_cookies", read_cookies)
            .map_err(|error| engine_error(&ctx, error))?;
        ctx.globals()
            .set("__neob_input_json", input_json)
            .map_err(|error| engine_error(&ctx, error))?;

        let prelude = ctx.eval::<(), _>(PRELUDE);
        if let Err(error) = prelude {
            return Err(map_error(&ctx, error, timeout, deadline));
        }
        if let Err(error) = ctx.eval::<(), _>(script) {
            let mut failure = map_error(&ctx, error, timeout, deadline);
            failure.logs = read_logs(&ctx);
            return Err(failure);
        }
        let output: String = match ctx.eval(EPILOGUE) {
            Ok(output) => output,
            Err(error) => {
                let mut failure = map_error(&ctx, error, timeout, deadline);
                failure.logs = read_logs(&ctx);
                return Err(failure);
            }
        };
        let logs = read_logs(&ctx);
        let parsed = parse_output(&output).map_err(|mut failure| {
            failure.logs = logs.clone();
            failure
        })?;
        Ok((parsed, logs))
    })
}

fn read_logs(ctx: &Ctx<'_>) -> Vec<String> {
    ctx.eval::<String, _>("JSON.stringify(__neob_logs)")
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn engine_error(ctx: &Ctx<'_>, failure: rquickjs::Error) -> ProcessorError {
    processor_error(ProcessorErrorKind::Engine, format_js_error(ctx, failure))
}

fn map_error(
    ctx: &Ctx<'_>,
    caught: rquickjs::Error,
    timeout: Duration,
    deadline: Instant,
) -> ProcessorError {
    if Instant::now() >= deadline {
        return processor_error(
            ProcessorErrorKind::Timeout,
            format!("processor exceeded {}ms", timeout.as_millis()),
        );
    }
    let message = format_js_error(ctx, caught);
    const INVALID: [&str; 10] = [
        "must return",
        "unknown return key",
        "must be synchronous",
        "headers must",
        "setHeaders must",
        "bodyBase64 must",
        "statusText must",
        "return either body",
        "must define function",
        "response body is binary",
    ];
    if INVALID.iter().any(|needle| message.contains(needle)) {
        return processor_error(ProcessorErrorKind::InvalidReturn, message);
    }
    if message.contains("out of memory") {
        return processor_error(ProcessorErrorKind::Engine, "out of memory");
    }
    processor_error(ProcessorErrorKind::Exception, message)
}

fn format_js_error(ctx: &Ctx<'_>, failure: rquickjs::Error) -> String {
    let value = ctx.catch();
    let mut message = failure.to_string();
    if let Some(object) = value.as_object() {
        if let Ok(text) = object.get::<_, String>("message") {
            message = text;
        }
        if let Ok(stack) = object.get::<_, String>("stack")
            && let Some(at) = stack.find("processor.js:")
        {
            let location = stack[at..].lines().next().unwrap_or_default();
            message = format!("{message} at {location}");
        }
    }
    message
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawResult {
    #[serde(default)]
    passthrough: bool,
    status: Option<i64>,
    status_text: Option<String>,
    headers: Option<Vec<(String, String)>>,
    set_headers: Option<BTreeMap<String, Option<OneOrMany>>>,
    body_text: Option<String>,
    body_base64: Option<String>,
}

fn parse_output(output: &str) -> Result<ProcessorResult, ProcessorError> {
    let raw: RawResult = serde_json::from_str(output)
        .map_err(|cause| processor_error(ProcessorErrorKind::InvalidReturn, cause.to_string()))?;
    if raw.passthrough {
        return Ok(ProcessorResult::Passthrough);
    }
    if let Some(status) = raw.status
        && !(200..=599).contains(&status)
    {
        return Err(processor_error(
            ProcessorErrorKind::InvalidReturn,
            "status must be 200-599",
        ));
    }
    if let Some(text) = &raw.status_text
        && (text.contains('\r') || text.contains('\n'))
    {
        return Err(processor_error(
            ProcessorErrorKind::InvalidReturn,
            "statusText must not contain CR or LF",
        ));
    }
    let headers = match raw.headers {
        Some(headers) => {
            for (name, value) in &headers {
                validate_header(name, value)?;
            }
            Some(
                headers
                    .into_iter()
                    .map(|(name, value)| HeaderPair::new(name, value))
                    .collect(),
            )
        }
        None => None,
    };
    let mut set_headers = BTreeMap::new();
    for (name, value) in raw.set_headers.unwrap_or_default() {
        let lower = name.to_ascii_lowercase();
        if set_headers.contains_key(&lower) {
            return Err(processor_error(
                ProcessorErrorKind::InvalidReturn,
                format!("duplicate setHeaders name: {lower}"),
            ));
        }
        if let Some(values) = &value {
            if values.to_vec().is_empty() {
                return Err(processor_error(
                    ProcessorErrorKind::InvalidReturn,
                    format!("setHeaders {lower} must not be empty"),
                ));
            }
            for value in values.to_vec() {
                validate_header(&name, &value)?;
            }
        }
        set_headers.insert(lower, value);
    }
    let body = match (raw.body_text, raw.body_base64.as_deref()) {
        (Some(text), _) => Some(text.into_bytes()),
        (None, Some(encoded)) => Some(base64_decode(encoded).ok_or_else(|| {
            processor_error(
                ProcessorErrorKind::InvalidReturn,
                "bodyBase64 is not base64",
            )
        })?),
        (None, None) => None,
    };
    if let Some(body) = &body
        && body.len() > MAX_OUTPUT_BYTES
    {
        return Err(processor_error(
            ProcessorErrorKind::OutputTooLarge,
            "processed body exceeds 8MiB",
        ));
    }
    let status = raw.status;
    if matches!(status, Some(204) | Some(304)) && body.as_ref().is_some_and(|body| !body.is_empty())
    {
        return Err(processor_error(
            ProcessorErrorKind::InvalidReturn,
            "204 and 304 must not carry a body",
        ));
    }
    Ok(ProcessorResult::Replace {
        status,
        status_text: raw.status_text,
        headers,
        set_headers,
        body,
    })
}

fn validate_header(name: &str, value: &str) -> Result<(), ProcessorError> {
    let invalid_name = name.is_empty() || name.contains(['\r', '\n', ':']);
    let invalid_value = value.contains(['\r', '\n']);
    if invalid_name || invalid_value {
        return Err(processor_error(
            ProcessorErrorKind::InvalidReturn,
            format!("invalid header {name}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn input() -> ProcessorInput {
        ProcessorInput {
            method: "GET".to_string(),
            url: "https://example.test/api/items?a=1&a=2&b=3".to_string(),
            request_headers: vec![HeaderPair::new("cookie", "a=1; b=2")],
            request_body: None,
            status: 200,
            status_text: "OK".to_string(),
            response_headers: vec![
                HeaderPair::new("content-type", "application/json"),
                HeaderPair::new("set-cookie", "x=1"),
                HeaderPair::new("set-cookie", "y=2"),
                HeaderPair::new("content-length", "12"),
            ],
            body: b"{\"ok\":true}".to_vec(),
            cookies_json: "[]".to_string(),
        }
    }

    async fn run(script: &str, input: ProcessorInput) -> Result<ProcessorRun, ProcessorError> {
        run_with(script, input, Box::new(|| Ok("[]".to_string()))).await
    }

    async fn run_with(
        script: &str,
        input: ProcessorInput,
        hook: Box<dyn Fn() -> Result<String, String> + Send + Sync>,
    ) -> Result<ProcessorRun, ProcessorError> {
        ScriptProcessor::new()
            .run(
                &ProcessorSpec {
                    script: script.to_string(),
                    timeout_ms: 200,
                },
                input,
                ProcessorHooks {
                    upstream_cookies: hook,
                },
            )
            .await
    }

    fn body(run: &ProcessorRun) -> String {
        match &run.result {
            ProcessorResult::Replace {
                body: Some(body), ..
            } => String::from_utf8(body.clone()).expect("utf8 body"),
            _ => String::new(),
        }
    }

    #[tokio::test]
    async fn process_returns_json_body() {
        let run = run("function process() { return { body: { a: 1 } } }", input())
            .await
            .expect("run");
        assert_eq!(body(&run), "{\"a\":1}");
    }

    #[tokio::test]
    async fn undefined_return_is_passthrough() {
        let run = run("function process() {}", input()).await.expect("run");
        assert!(matches!(run.result, ProcessorResult::Passthrough));
    }

    #[tokio::test]
    async fn async_process_is_invalid() {
        let failure = run("async function process() { return {} }", input())
            .await
            .expect_err("async is invalid");
        assert_eq!(
            failure.kind,
            ProcessorErrorKind::InvalidReturn,
            "{}",
            failure.message
        );
    }

    #[tokio::test]
    async fn unknown_return_key_is_invalid() {
        let failure = run("function process() { return { statusCode: 200 } }", input())
            .await
            .expect_err("unknown key");
        assert_eq!(failure.kind, ProcessorErrorKind::InvalidReturn);
        assert!(
            failure.message.contains("unknown return key"),
            "{}",
            failure.message
        );
    }

    #[tokio::test]
    async fn headers_must_be_pairs() {
        let failure = run(
            "function process() { return { headers: { a: 1 } } }",
            input(),
        )
        .await
        .expect_err("object headers");
        assert_eq!(failure.kind, ProcessorErrorKind::InvalidReturn);
    }

    #[tokio::test]
    async fn header_with_newline_is_rejected() {
        let script = r#"function process() { return { setHeaders: { 'x-a': 'a\nb' } } }"#;
        let failure = run(script, input()).await.expect_err("newline");
        assert_eq!(failure.kind, ProcessorErrorKind::InvalidReturn);
    }

    #[tokio::test]
    async fn set_cookie_can_be_added_removed_and_replaced() {
        let script = r#"function process() {
            return { setHeaders: { 'set-cookie': ['x=2; Path=/', 'z=1; Path=/'] } };
        }"#;
        let added = run(script, input()).await.expect("run");
        let ProcessorResult::Replace { set_headers, .. } = added.result else {
            panic!("expected replace");
        };
        let values = set_headers
            .get("set-cookie")
            .and_then(|value| value.as_ref())
            .map(|value| value.to_vec())
            .unwrap_or_default();
        assert_eq!(values, vec!["x=2; Path=/", "z=1; Path=/"]);

        let remove = run(
            "function process() { return { setHeaders: { 'set-cookie': null } } }",
            input(),
        )
        .await
        .expect("run");
        let ProcessorResult::Replace { set_headers, .. } = remove.result else {
            panic!("expected replace");
        };
        assert!(set_headers.get("set-cookie").is_some_and(Option::is_none));
    }

    #[tokio::test]
    async fn ctx_helpers_read_headers_and_cookies() {
        let script = r#"function process({ request, response }) {
            return { body: {
                cookies: response.headerAll('set-cookie').length,
                second: response.setCookies()[1].name,
                requestCookie: request.cookies().b,
                cookieLines: request.headerAll('cookie').length,
            } };
        }"#;
        let run = run(script, input()).await.expect("run");
        let value: serde_json::Value = serde_json::from_str(&body(&run)).expect("json");
        assert_eq!(value["cookies"], 2);
        assert_eq!(value["second"], "y");
        assert_eq!(value["requestCookie"], "2");
        assert_eq!(value["cookieLines"], 1);
    }

    #[tokio::test]
    async fn ctx_cookies_calls_the_hook_and_caches() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let hook = Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok("[{\"name\":\"up1\",\"value\":\"1\",\"path\":\"/\"}]".to_string())
        });
        let script = r#"function process({ response }) {
            const first = response.cookies();
            const second = response.cookies();
            return { body: { same: first[0].name === second[0].name, length: first.length } };
        }"#;
        let run = run_with(script, input(), hook).await.expect("run");
        let value: serde_json::Value = serde_json::from_str(&body(&run)).expect("json");
        assert_eq!(value["same"], true);
        assert_eq!(value["length"], 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn ctx_cookies_empty_when_hook_fails() {
        let hook = Box::new(|| Err("no cookies".to_string()));
        let script = "function process({ response }) { return { body: { length: response.cookies().length } } }";
        let run = run_with(script, input(), hook).await.expect("run");
        let value: serde_json::Value = serde_json::from_str(&body(&run)).expect("json");
        assert_eq!(value["length"], 0);
    }

    #[tokio::test]
    async fn ctx_query_groups_repeated_keys() {
        let script = "function process({ request }) { return { body: { query: request.query } } }";
        let run = run(script, input()).await.expect("run");
        let value: serde_json::Value = serde_json::from_str(&body(&run)).expect("json");
        assert_eq!(value["query"]["a"], serde_json::json!(["1", "2"]));
        assert_eq!(value["query"]["b"], serde_json::json!(["3"]));
    }

    #[tokio::test]
    async fn infinite_loop_times_out() {
        let started = Instant::now();
        let failure = ScriptProcessor::new()
            .run(
                &ProcessorSpec {
                    script: "function process() { while (true) {} }".to_string(),
                    timeout_ms: 50,
                },
                input(),
                ProcessorHooks {
                    upstream_cookies: Box::new(|| Ok("[]".to_string())),
                },
            )
            .await
            .expect_err("timeout");
        assert_eq!(failure.kind, ProcessorErrorKind::Timeout);
        assert!(started.elapsed() < Duration::from_millis(300));
    }

    #[tokio::test]
    async fn console_logs_are_captured_and_capped() {
        let script = r#"function process() {
            for (let i = 0; i < 60; i += 1) console.log('line ' + i);
        }"#;
        let run = run(script, input()).await.expect("run");
        assert_eq!(run.logs.len(), 50);
        assert_eq!(run.logs[0], "line 0");
    }

    #[tokio::test]
    async fn binary_body_exposes_base64_only() {
        let mut input = input();
        input.body = vec![0xff, 0xfe, 0x01];
        let script = r#"function process({ response }) {
            let jsonThrows = false;
            try { response.json(); } catch (_) { jsonThrows = true; }
            return { body: { text: response.body, base64: response.bodyBase64 !== null, jsonThrows } };
        }"#;
        let run = run(script, input).await.expect("run");
        let value: serde_json::Value = serde_json::from_str(&body(&run)).expect("json");
        assert!(value["text"].is_null());
        assert_eq!(value["base64"], true);
        assert_eq!(value["jsonThrows"], true);
    }

    #[tokio::test]
    async fn exception_reports_script_line() {
        let failure = run(
            "\n\nthrow new Error('boom')\nfunction process() {}",
            input(),
        )
        .await
        .expect_err("throws");
        assert_eq!(failure.kind, ProcessorErrorKind::Exception);
        assert!(failure.message.contains("boom"), "{}", failure.message);
    }

    #[test]
    fn missing_process_function_fails_validation() {
        assert!(validate_script("const a = 1").is_err());
        assert!(validate_script("function process() {}").is_ok());
    }

    #[tokio::test]
    async fn no_io_globals() {
        let script = r#"function process() {
            return { body: {
                fetch: typeof fetch,
                setTimeout: typeof setTimeout,
                require: typeof require,
                atob: typeof atob,
            } };
        }"#;
        let run = run(script, input()).await.expect("run");
        let value: serde_json::Value = serde_json::from_str(&body(&run)).expect("json");
        assert_eq!(value["fetch"], "undefined");
        assert_eq!(value["setTimeout"], "undefined");
        assert_eq!(value["require"], "undefined");
        assert_eq!(value["atob"], "function");
    }
}
