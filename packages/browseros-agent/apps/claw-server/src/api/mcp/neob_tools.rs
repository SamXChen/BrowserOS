//! The tools NeoB adds on top of the shared browser set.
//!
//! Three places have to agree: this registry, the `## NeoB-only tools` section of
//! `skills/neob/SKILL.md`, and `EXPECTED_TOOLS` in the MCP contract. The checks below
//! run them against each other, and one reverse test proves a missing step fails. Adding
//! a tool therefore cannot quietly ship an undocumented or unlisted capability.

/// One NeoB-only tool: the name, and the sentence the skill shows the agent.
pub struct NeobTool {
    pub name: &'static str,
    pub when: &'static str,
}

/// Tools that exist only in this build. Keep the list short: everything that upstream
/// also ships belongs in the shared catalogue, not here.
pub const NEOB_ONLY_TOOLS: &[NeobTool] = &[NeobTool {
    name: "network",
    when: "capture traffic, read a request the user pasted as `netref_*`, and install or edit mock rules",
}];

#[cfg(test)]
const SKILL: &str = include_str!("../../../../../../../skills/neob/SKILL.md");
#[cfg(test)]
const CONTRACT: &str = include_str!("../../../../../contracts/claw-mcp/tests/cases-transport.ts");

/// Why a registration is inconsistent. The message names the document to edit so the
/// failure is actionable without reading this module.
#[derive(Debug, PartialEq, Eq)]
pub enum RegistryProblem {
    NotInCatalog(String),
    MissingSkillLine(String),
    SkillLineDiffers(String),
    ExtraSkillLine(String),
    MissingInContract(String),
    EmptyWhen(String),
}

impl std::fmt::Display for RegistryProblem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInCatalog(name) => write!(
                formatter,
                "`{name}` is registered as a NeoB-only tool but tools/list never exposes it"
            ),
            Self::MissingSkillLine(name) => write!(
                formatter,
                "`{name}` is missing from `## NeoB-only tools` in skills/neob/SKILL.md"
            ),
            Self::SkillLineDiffers(name) => write!(
                formatter,
                "skills/neob/SKILL.md describes `{name}` differently from NEOB_ONLY_TOOLS"
            ),
            Self::ExtraSkillLine(name) => write!(
                formatter,
                "skills/neob/SKILL.md lists `{name}`, which is not in NEOB_ONLY_TOOLS (design/adding-neob-tools.md)"
            ),
            Self::MissingInContract(name) => write!(
                formatter,
                "`{name}` is missing from EXPECTED_TOOLS in contracts/claw-mcp/tests/cases-transport.ts"
            ),
            Self::EmptyWhen(name) => write!(
                formatter,
                "`{name}` has no `when` sentence; the skill needs one line telling the agent when to use it"
            ),
        }
    }
}

/// The bullet lines under `## NeoB-only tools`, as `(name, when)` pairs, read from the
/// skill exactly as an agent sees it: `` - `network`: capture traffic … “.
#[cfg(test)]
fn skill_section() -> Vec<(String, String)> {
    let start = match SKILL.find("## NeoB-only tools") {
        Some(index) => index,
        None => return Vec::new(),
    };
    let rest = &SKILL[start..];
    let end = rest[1..].find("\n## ").map_or(rest.len(), |offset| offset + 1);
    rest[..end]
        .lines()
        .filter_map(|line| line.strip_prefix("- `"))
        .filter_map(|line| {
            let (name, when) = line.split_once("`: ")?;
            Some((name.to_string(), when.to_string()))
        })
        .collect()
}

/// Checks a registry against the catalogue, the skill and the contract. Split out so the
/// reverse test can feed it deliberately broken inputs.
#[cfg(test)]
fn check(
    registry: &[NeobTool],
    catalog_name_list: &[&str],
    skill_list: &[(String, String)],
    contract: &str,
) -> Vec<RegistryProblem> {
    let mut problems = Vec::new();
    for tool in registry {
        if tool.when.trim().is_empty() {
            problems.push(RegistryProblem::EmptyWhen(tool.name.to_string()));
        }
        if !catalog_name_list.contains(&tool.name) {
            problems.push(RegistryProblem::NotInCatalog(tool.name.to_string()));
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

    use super::{CONTRACT, NEOB_ONLY_TOOLS, RegistryProblem, check, skill_section};

    fn catalog_name_list() -> Vec<&'static str> {
        catalog().into_iter().map(|tool| tool.name).collect()
    }

    #[test]
    fn the_registry_matches_the_catalogue_the_skill_and_the_contract() {
        let catalog_name_list = catalog_name_list();
        let problems = check(
            NEOB_ONLY_TOOLS,
            &catalog_name_list,
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

    /// The guard has to fail when a step is skipped, otherwise it is decoration.
    #[test]
    fn a_missed_step_is_reported() {
        let catalog_name_list = catalog_name_list();

        let unlisted = [super::NeobTool { name: "tabs", when: "" }];
        let problems = check(&unlisted, &catalog_name_list, &skill_section(), CONTRACT);
        assert!(problems.contains(&RegistryProblem::EmptyWhen("tabs".into())));
        assert!(problems.contains(&RegistryProblem::MissingSkillLine("tabs".into())));

        let unknown = [super::NeobTool { name: "made-up-tool", when: "never" }];
        let problems = check(&unknown, &catalog_name_list, &skill_section(), CONTRACT);
        assert!(problems.contains(&RegistryProblem::NotInCatalog("made-up-tool".into())));

        let contractless = [super::NeobTool { name: "network", when: "capture traffic" }];
        let problems = check(
            &contractless,
            &catalog_name_list,
            &skill_section(),
            "const EXPECTED_TOOLS = [];",
        );
        assert!(problems.contains(&RegistryProblem::MissingInContract("network".into())));

        let extra = vec![
            ("network".to_string(), NEOB_ONLY_TOOLS[0].when.to_string()),
            ("ghost".to_string(), "never".to_string()),
        ];
        let problems = check(NEOB_ONLY_TOOLS, &catalog_name_list, &extra, CONTRACT);
        assert!(problems.contains(&RegistryProblem::ExtraSkillLine("ghost".into())));

        let reworded = vec![("network".to_string(), "something else".to_string())];
        let problems = check(NEOB_ONLY_TOOLS, &catalog_name_list, &reworded, CONTRACT);
        assert!(problems.contains(&RegistryProblem::SkillLineDiffers("network".into())));
    }
}
