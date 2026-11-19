//! Guards the wording agents actually read.
//!
//! Every string here is assembled from a constant or a tool definition, so the checks run
//! without building a service. `BrowserOS` is accepted where the text names the kernel or
//! its version, and rejected where it would read as a self-name.

#![cfg(test)]

use browseros_core::NativeCapabilities;

use crate::config::PursueOffering;
use crate::services::agent_settings::HumanHelp;

use super::prompt::mcp_instructions;

/// Names that belong to another build or another product. A self-name or a trigger word
/// must never be one of these: the agent would call NeoB by a name the user did not use,
/// and a second installed browser skill would compete for the same trigger.
const FORBIDDEN_TEXT_LIST: [&str; 8] = [
    "BrowserOS neo",
    "browseros-neo",
    "browserclaw",
    "BrowserClaw",
    "\"use neo\"",
    "neo-<name>",
    "/neo;",
    "a neo task",
];

/// Prefixes of a `BrowserOS` occurrence that describe the kernel: version statements, and
/// the one feature-gating sentence that says a method needs a newer kernel.
const KERNEL_CONTEXT_LIST: [&str; 3] = [
    "BrowserOS 0.",
    "BrowserOS {version}",
    "BrowserOS return native_feature_unavailable",
];

/// How much of the surrounding text a violation message quotes, so a failure is readable
/// without printing a whole tool description.
const VIOLATION_PREVIEW_LEN: usize = 60;

#[test]
fn agent_visible_text_uses_only_the_neob_identity() {
    let mut violation_list = Vec::new();
    let text_list = instructions_corpus()
        .into_iter()
        .chain(tool_corpus())
        .chain(super::service::local_agent_visible_text());

    for (source, text) in text_list {
        for needle in FORBIDDEN_TEXT_LIST {
            if text.contains(needle) {
                violation_list.push(format!("{source}: {needle}"));
            }
        }
        collect_bare_kernel_names(source, &text, &mut violation_list);
    }

    assert!(
        violation_list.is_empty(),
        "agent-visible text still names another product: {violation_list:#?}"
    );
}

#[test]
fn agent_visible_text_introduces_neob() {
    let instructions = mcp_instructions(
        HumanHelp::On,
        false,
        PursueOffering::Absent,
        NativeCapabilities::from_browseros_version(Some("0.51.1.3")),
    );
    assert!(
        instructions.contains("NeoB"),
        "instructions must introduce NeoB"
    );
    assert!(
        instructions.contains("`neob` server for the whole task"),
        "instructions must scope the whole task to the neob server"
    );
    assert_eq!(crate::identity::DISPLAY_NAME, "NeoB");
}

/// Every `BrowserOS` in `text` must continue as one of the kernel phrasings.
fn collect_bare_kernel_names(source: &str, text: &str, violation_list: &mut Vec<String>) {
    let mut rest = text;
    while let Some(index) = rest.find("BrowserOS") {
        let tail = &rest[index..];
        let describes_kernel = KERNEL_CONTEXT_LIST
            .iter()
            .any(|allowed| tail.starts_with(allowed));
        if !describes_kernel {
            let preview = &tail[..tail.len().min(VIOLATION_PREVIEW_LEN)];
            violation_list.push(format!("{source}: bare BrowserOS -> {preview}"));
        }
        rest = &tail["BrowserOS".len()..];
    }
}

/// One entry per instruction combination the server can send.
fn instructions_corpus() -> Vec<(&'static str, String)> {
    let native = NativeCapabilities::from_browseros_version(Some("0.51.1.3"));
    let mut text_list = Vec::new();
    for human_help in [HumanHelp::On, HumanHelp::Off] {
        for close_groups in [false, true] {
            for pursue in [PursueOffering::Absent, PursueOffering::Offered] {
                text_list.push((
                    "instructions",
                    mcp_instructions(human_help, close_groups, pursue, native),
                ));
            }
        }
    }
    text_list
}

/// One entry per tool the shared catalogue defines.
fn tool_corpus() -> Vec<(&'static str, String)> {
    browseros_mcp::tools::catalog()
        .into_iter()
        .map(|tool| (tool.name, tool.description.to_string()))
        .collect()
}
