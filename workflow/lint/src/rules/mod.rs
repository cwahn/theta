use std::path::Path;

mod cfg_order;
mod common;
mod path_depth;
mod sort_use;
mod use_group_order;
mod use_toplevel;

pub struct Violation {
    pub rule: &'static str,
    pub file: std::path::PathBuf,
    pub line: usize,
    pub message: String,
}

pub trait Rule: Send + Sync {
    fn name(&self) -> &'static str;
    /// Returns (line, message) pairs for each violation found.
    fn check(&self, file: &syn::File) -> Vec<(usize, String)>;
}

static RULES: &[&dyn Rule] = &[
    &cfg_order::CfgOrder,
    &path_depth::PathDepth,
    &sort_use::SortUse,
    &use_group_order::UseGroupOrder,
    &use_toplevel::UseToplevel,
];

pub fn run_all(file_path: &Path, source: &str) -> Vec<Violation> {
    let Ok(parsed) = syn::parse_file(source) else {
        return vec![];
    };

    let mut violations = vec![];
    for rule in RULES {
        for (line, message) in rule.check(&parsed) {
            violations.push(Violation {
                rule: rule.name(),
                file: file_path.to_path_buf(),
                line,
                message,
            });
        }
    }
    violations
}
