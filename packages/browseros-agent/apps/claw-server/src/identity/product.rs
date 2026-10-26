//! The product identity of NeoB.
//!
//! Every agent-facing name and every resource this server owns derives from here, so a
//! rename is one edit. The naming rules are:
//!
//! - Agent-facing text calls this tool `NeoB` only. The trigger words are `neob` and
//!   `NeoB` (the slug already contains the short form).
//! - `BrowserOS` may appear only when describing the kernel or its version, because the
//!   kernel really is the upstream BrowserOS build. `BrowserOS neo`, `browserclaw` and a
//!   bare `neo` are never used as a self-name or as a trigger.
//! - This server manages exactly one MCP entry name and one skill name: [`SLUG`]. It
//!   never reads, rewrites or removes an entry or directory it does not own.

/// Macro form of [`DISPLAY_NAME`], for constants that must be built with `concat!`,
/// which only accepts literals — a `const &str` cannot be interpolated into it.
#[macro_export]
macro_rules! product_name {
    () => {
        "NeoB"
    };
}

/// Skill name, skill directory name, MCP server name, the MCP entry name written into
/// every harness config, the ownership marker, and the reserved skill name.
pub const SLUG: &str = "neob";

/// How the product introduces itself to agents.
pub const DISPLAY_NAME: &str = product_name!();

/// Data directory under `$HOME`. Kept separate from any other build of this server so
/// the two can never migrate or reconcile each other's data.
pub const DATA_DIR_NAME: &str = ".neob";
pub const DEV_DATA_DIR_NAME: &str = ".neob-dev";

/// Namespace for skills saved through `save_skill`, so `/neob` lists them and they never
/// collide with a skill saved by another build.
pub const SAVED_SKILL_PREFIX: &str = "neob-";

#[cfg(test)]
mod tests {
    use super::{DATA_DIR_NAME, DISPLAY_NAME, SAVED_SKILL_PREFIX, SLUG};

    /// Agent Skills requires `^[a-z0-9]+(-[a-z0-9]+)*$`, at most 64 characters, and the
    /// skill directory must use the same name.
    #[test]
    fn slug_is_a_valid_skill_name() {
        assert!(SLUG.len() <= 64);
        assert!(!SLUG.is_empty());
        assert!(SLUG
            .split('-')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())));
    }

    /// `/neob` must list the product skill and every saved skill in one namespace.
    #[test]
    fn saved_skills_share_the_product_prefix() {
        assert_eq!(SAVED_SKILL_PREFIX, format!("{SLUG}-"));
    }

    #[test]
    fn identity_does_not_reuse_another_builds_names() {
        assert_ne!(SLUG, "browseros-neo");
        assert_ne!(SLUG, "browserclaw");
        assert_ne!(DATA_DIR_NAME, ".browserclaw");
        assert_eq!(DISPLAY_NAME, "NeoB");
    }
}
