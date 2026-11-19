//! The tools NeoB adds on top of the shared browser set.
//!
//! The registry, catalog, guides, skill section and MCP contract are checked in
//! both directions. Upstream tools are explicitly classified so a newly added
//! NeoB tool cannot silently skip the registration and documentation steps.

/// One NeoB-only tool: its name, the sentence shown in the skill, and its guide.
#[derive(Clone, Copy)]
pub struct NeobTool {
    pub name: &'static str,
    pub when: &'static str,
    pub guide: &'static str,
}

/// Tools that exist only in this build.
pub const NEOB_ONLY_TOOLS: &[NeobTool] = &[NeobTool {
    name: "network",
    when: "capture traffic, read a request the user pasted as `netref_*`, and install or edit mock rules",
    guide: browseros_mcp::tools::network::GUIDE,
}];

/// Tools inherited from upstream. Feature-gated tools may be absent from a
/// particular tools/list response, but every exposed catalog tool must be in
/// either this list or `NEOB_ONLY_TOOLS`.
pub const UPSTREAM_TOOLS: &[&str] = &[
    "act",
    "await_human_help",
    "browser_profiles",
    "browser_target",
    "diff",
    "download",
    "evaluate",
    "grep",
    "history",
    "mark_skill_run",
    "name_session",
    "navigate",
    "pdf",
    "pursue",
    "read",
    "request_human_help",
    "run",
    "save_skill",
    "screenshot",
    "snapshot",
    "tab_groups",
    "tabs",
    "upload",
    "wait",
    "windows",
];

#[cfg(test)]
const SKILL: &str = include_str!("../../../../../../../skills/neob/SKILL.md");
#[cfg(test)]
const CONTRACT: &str = include_str!("../../../../../contracts/claw-mcp/tests/cases-transport.ts");

#[derive(Debug, PartialEq, Eq)]
pub enum RegistryProblem {
    NotInCatalog(String),
    UnclassifiedCatalogTool(String),
    RegistryUpstreamOverlap(String),
    MissingSkillLine(String),
    SkillLineDiffers(String),
    ExtraSkillLine(String),
    MissingInContract(String),
    EmptyWhen(String),
    EmptyGuide(String),
    GuideTooLarge(String),
    GuideMismatch(String),
}

impl std::fmt::Display for RegistryProblem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::NotInCatalog(name) => format!(
                "`{name}` is registered as a NeoB-only tool but tools/list never exposes it"
            ),
            Self::UnclassifiedCatalogTool(name) => {
                format!("catalog tool `{name}` is in neither NEOB_ONLY_TOOLS nor UPSTREAM_TOOLS")
            }
            Self::RegistryUpstreamOverlap(name) => {
                format!("`{name}` is listed as both a NeoB-only tool and an upstream tool")
            }
            Self::MissingSkillLine(name) => {
                format!("`{name}` is missing from `## NeoB-only tools` in skills/neob/SKILL.md")
            }
            Self::SkillLineDiffers(name) => {
                format!("skills/neob/SKILL.md describes `{name}` differently from NEOB_ONLY_TOOLS")
            }
            Self::ExtraSkillLine(name) => {
                format!("skills/neob/SKILL.md lists `{name}`, which is not in NEOB_ONLY_TOOLS")
            }
            Self::MissingInContract(name) => format!(
                "`{name}` is missing from EXPECTED_TOOLS in contracts/claw-mcp/tests/cases-transport.ts"
            ),
            Self::EmptyWhen(name) => {
                format!("`{name}` has no `when` sentence for the NeoB-only tools skill section")
            }
            Self::EmptyGuide(name) => format!("NeoB-only tool `{name}` has an empty guide"),
            Self::GuideTooLarge(name) => {
                format!("NeoB-only tool `{name}` has a guide larger than 3072 bytes")
            }
            Self::GuideMismatch(name) => format!(
                "NeoB-only tool `{name}` does not expose its registered guide as its description"
            ),
        };
        write!(formatter, "{message} (design/adding-neob-tools.md)")
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct CatalogEntry<'a> {
    name: &'a str,
    description: &'a str,
}

/// The bullet lines under `## NeoB-only tools`, as `(name, when)` pairs.
#[cfg(test)]
fn skill_section() -> Vec<(String, String)> {
    let start = match SKILL.find("## NeoB-only tools") {
        Some(index) => index,
        None => return Vec::new(),
    };
    let rest = &SKILL[start..];
    let end = rest[1..]
        .find("\n## ")
        .map_or(rest.len(), |offset| offset + 1);
    rest[..end]
        .lines()
        .filter_map(|line| line.strip_prefix("- `"))
        .filter_map(|line| {
            let (name, when) = line.split_once("`: ")?;
            Some((name.to_string(), when.to_string()))
        })
        .collect()
}

/// Check every registration direction. Broken inputs are injected by the reverse
/// test so each required edit has a demonstrated failure mode.
#[cfg(test)]
fn check(
    registry: &[NeobTool],
    upstream: &[&str],
    catalog: &[CatalogEntry<'_>],
    skill_list: &[(String, String)],
    contract: &str,
) -> Vec<RegistryProblem> {
    let mut problems = Vec::new();
    for tool in registry {
        if upstream.contains(&tool.name) {
            problems.push(RegistryProblem::RegistryUpstreamOverlap(
                tool.name.to_string(),
            ));
        }
        if tool.when.trim().is_empty() {
            problems.push(RegistryProblem::EmptyWhen(tool.name.to_string()));
        }
        if tool.guide.trim().is_empty() {
            problems.push(RegistryProblem::EmptyGuide(tool.name.to_string()));
        }
        if tool.guide.len() > 3 * 1024 {
            problems.push(RegistryProblem::GuideTooLarge(tool.name.to_string()));
        }
        match catalog.iter().find(|entry| entry.name == tool.name) {
            Some(entry) if entry.description != tool.guide => {
                problems.push(RegistryProblem::GuideMismatch(tool.name.to_string()));
            }
            Some(_) => {}
            None => problems.push(RegistryProblem::NotInCatalog(tool.name.to_string())),
        }
        match skill_list.iter().find(|(name, _)| name == tool.name) {
            Some((_, when)) if when.trim() == tool.when.trim() => {}
            Some(_) => problems.push(RegistryProblem::SkillLineDiffers(tool.name.to_string())),
            None => problems.push(RegistryProblem::MissingSkillLine(tool.name.to_string())),
        }
        let expected = format!("'{}',", tool.name);
        if !contract.contains(&expected) {
            problems.push(RegistryProblem::MissingInContract(tool.name.to_string()));
        }
    }
    for entry in catalog {
        if !registry.iter().any(|tool| tool.name == entry.name) && !upstream.contains(&entry.name) {
            problems.push(RegistryProblem::UnclassifiedCatalogTool(
                entry.name.to_string(),
            ));
        }
    }
    for (name, _) in skill_list {
        if !registry.iter().any(|tool| tool.name == name) {
            problems.push(RegistryProblem::ExtraSkillLine(name.clone()));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use browseros_mcp::tools::catalog;

    use super::{
        CONTRACT, CatalogEntry, NEOB_ONLY_TOOLS, NeobTool, RegistryProblem, SKILL, UPSTREAM_TOOLS,
        check, skill_section,
    };

    fn catalog_entries() -> Vec<CatalogEntry<'static>> {
        catalog()
            .into_iter()
            .map(|tool| CatalogEntry {
                name: tool.name,
                description: tool.description,
            })
            .collect()
    }

    #[test]
    fn registry_matches_catalog() {
        let problems = check(
            NEOB_ONLY_TOOLS,
            UPSTREAM_TOOLS,
            &catalog_entries(),
            &skill_section(),
            CONTRACT,
        );
        assert!(
            problems.is_empty(),
            "NeoB-only tool registration is inconsistent:\n{}",
            problems
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn every_neob_tool_has_a_guide() {
        let catalog = catalog_entries();
        for tool in NEOB_ONLY_TOOLS {
            let entry = catalog
                .iter()
                .find(|entry| entry.name == tool.name)
                .unwrap_or_else(|| panic!("{} is missing from the catalog", tool.name));
            assert!(!tool.guide.trim().is_empty());
            assert!(tool.guide.len() <= 3 * 1024);
            assert_eq!(entry.description, tool.guide);
        }
    }

    #[test]
    fn skill_section_lists_exactly_the_registry() {
        let skill = skill_section();
        let expected = NEOB_ONLY_TOOLS
            .iter()
            .map(|tool| (tool.name.to_string(), tool.when.to_string()))
            .collect::<Vec<_>>();
        assert_eq!(skill, expected);
    }

    #[test]
    fn contract_lists_every_neob_tool() {
        for tool in NEOB_ONLY_TOOLS {
            assert!(CONTRACT.contains(&format!("'{}',", tool.name)));
        }
    }

    #[test]
    fn a_missed_step_is_reported() {
        let catalog = catalog_entries();
        let mut all_problems = Vec::new();
        let mut run = |registry: &[NeobTool],
                       upstream: &[&str],
                       catalog: &[CatalogEntry<'_>],
                       skill: &[(String, String)],
                       contract: &str| {
            let problems = check(registry, upstream, catalog, skill, contract);
            assert!(
                !problems.is_empty(),
                "broken registration unexpectedly passed"
            );
            all_problems.extend(problems);
        };

        let unknown = [NeobTool {
            name: "made-up-tool",
            when: "never",
            guide: "guide",
        }];
        let problems = check(
            &unknown,
            UPSTREAM_TOOLS,
            &catalog,
            &skill_section(),
            CONTRACT,
        );
        assert!(problems.contains(&RegistryProblem::NotInCatalog("made-up-tool".into())));

        let mut unclassified_catalog = catalog.clone();
        unclassified_catalog.push(CatalogEntry {
            name: "unclassified-tool",
            description: "guide",
        });
        let problems = check(
            NEOB_ONLY_TOOLS,
            UPSTREAM_TOOLS,
            &unclassified_catalog,
            &skill_section(),
            CONTRACT,
        );
        assert!(problems.contains(&RegistryProblem::UnclassifiedCatalogTool(
            "unclassified-tool".into()
        )));

        let problems = check(
            NEOB_ONLY_TOOLS,
            &["network"],
            &catalog,
            &skill_section(),
            CONTRACT,
        );
        assert!(problems.contains(&RegistryProblem::RegistryUpstreamOverlap("network".into())));

        let empty = [NeobTool {
            name: "network",
            when: "",
            guide: "",
        }];
        let problems = check(&empty, UPSTREAM_TOOLS, &catalog, &[], CONTRACT);
        assert!(problems.contains(&RegistryProblem::EmptyWhen("network".into())));
        assert!(problems.contains(&RegistryProblem::EmptyGuide("network".into())));
        assert!(problems.contains(&RegistryProblem::GuideMismatch("network".into())));
        assert!(problems.contains(&RegistryProblem::MissingSkillLine("network".into())));

        let wrong_guide = [NeobTool {
            guide: "wrong guide",
            ..NEOB_ONLY_TOOLS[0]
        }];
        let problems = check(
            &wrong_guide,
            UPSTREAM_TOOLS,
            &catalog,
            &skill_section(),
            CONTRACT,
        );
        assert!(problems.contains(&RegistryProblem::GuideMismatch("network".into())));

        let too_large_guide = Box::leak("x".repeat(3 * 1024 + 1).into_boxed_str());
        let too_large = [NeobTool {
            guide: too_large_guide,
            ..NEOB_ONLY_TOOLS[0]
        }];
        let problems = check(
            &too_large,
            UPSTREAM_TOOLS,
            &catalog,
            &skill_section(),
            CONTRACT,
        );
        assert!(problems.contains(&RegistryProblem::GuideTooLarge("network".into())));

        let problems = check(
            NEOB_ONLY_TOOLS,
            UPSTREAM_TOOLS,
            &catalog,
            &skill_section(),
            "const EXPECTED_TOOLS = [];",
        );
        assert!(problems.contains(&RegistryProblem::MissingInContract("network".into())));

        let extra = vec![
            ("network".to_string(), NEOB_ONLY_TOOLS[0].when.to_string()),
            ("ghost".to_string(), "never".to_string()),
        ];
        let problems = check(NEOB_ONLY_TOOLS, UPSTREAM_TOOLS, &catalog, &extra, CONTRACT);
        assert!(problems.contains(&RegistryProblem::ExtraSkillLine("ghost".into())));

        let reworded = vec![("network".to_string(), "something else".to_string())];
        let problems = check(
            NEOB_ONLY_TOOLS,
            UPSTREAM_TOOLS,
            &catalog,
            &reworded,
            CONTRACT,
        );
        assert!(problems.contains(&RegistryProblem::SkillLineDiffers("network".into())));

        run(
            &unknown,
            UPSTREAM_TOOLS,
            &catalog,
            &skill_section(),
            CONTRACT,
        );
        for problem in all_problems {
            assert!(
                problem.to_string().contains("design/adding-neob-tools.md"),
                "error lacks the remediation guide: {problem}"
            );
        }
        assert!(SKILL.contains("## NeoB-only tools"));
    }
}
