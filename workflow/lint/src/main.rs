mod rules;

use std::{path::Path, process};

use walkdir::WalkDir;

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("lint") => {
            let root = std::env::current_dir().expect("failed to get cwd");
            let violations = lint_workspace(&root);

            for v in &violations {
                eprintln!(
                    "[{}] {}:{} — {}",
                    v.rule,
                    v.file.display(),
                    v.line,
                    v.message
                );
            }

            if !violations.is_empty() {
                process::exit(1);
            }
        }
        _ => {
            eprintln!("Usage: theta-lint lint");
            process::exit(1);
        }
    }
}

fn lint_workspace(root: &Path) -> Vec<rules::Violation> {
    let scan_dirs: &[&str] = &["theta", "theta-macros", "theta-ts", "examples", "workflow/lint"];

    let mut violations = vec![];

    for dir in scan_dirs {
        let base = root.join(dir);
        if !base.exists() {
            continue;
        }

        for entry in WalkDir::new(&base)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| is_rust_source(e.path()))
        {
            let path = entry.path();
            let Ok(source) = std::fs::read_to_string(path) else {
                continue;
            };
            violations.extend(rules::run_all(path, &source));
        }
    }

    violations.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.rule.cmp(b.rule))
    });

    violations
}

fn is_rust_source(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("rs")
        && !path
            .components()
            .any(|c| c.as_os_str() == "target" || c.as_os_str() == "build")
}
