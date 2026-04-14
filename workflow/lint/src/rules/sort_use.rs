use std::collections::HashMap;

use proc_macro2::Span;
use syn::{Item, UseTree};

use super::{Rule, common};

pub struct SortUse;

fn span_line(span: Span) -> usize {
    span.start().line
}

/// Return the origin of the top-level path in a UseTree.
fn tree_origin(tree: &UseTree) -> common::Origin {
    match tree {
        UseTree::Path(p) => common::classify_root(&p.ident.to_string()),
        UseTree::Name(n) => common::classify_root(&n.ident.to_string()),
        UseTree::Rename(r) => common::classify_root(&r.ident.to_string()),
        UseTree::Group(g) => g
            .items
            .iter()
            .map(tree_origin)
            .min()
            .unwrap_or(common::Origin::External),
        UseTree::Glob(_) => common::Origin::External,
    }
}

/// Return the first path segment of a UseTree.
fn first_segment(tree: &UseTree) -> Option<String> {
    match tree {
        UseTree::Path(p) => Some(p.ident.to_string()),
        UseTree::Name(n) => Some(n.ident.to_string()),
        UseTree::Rename(r) => Some(r.ident.to_string()),
        UseTree::Group(g) => g.items.first().and_then(first_segment),
        UseTree::Glob(_) => None,
    }
}

/// Recursively check that no sibling items in a group share a first segment.
fn check_tree(tree: &UseTree, prefix: &str, violations: &mut Vec<(usize, String)>, line: usize) {
    match tree {
        UseTree::Group(group) => {
            let mut root_counts: HashMap<String, usize> = HashMap::new();
            for sub in &group.items {
                if let Some(root) = first_segment(sub) {
                    *root_counts.entry(root).or_insert(0) += 1;
                }
            }
            let mut dups: Vec<&String> = root_counts
                .iter()
                .filter(|&(_, &c)| c > 1)
                .map(|(k, _)| k)
                .collect();
            if !dups.is_empty() {
                dups.sort();
                let context = if prefix.is_empty() {
                    "use { }".to_string()
                } else {
                    format!("use {prefix}::{{ }}")
                };
                violations.push((
                    line,
                    format!(
                        "{context} has items with shared root that should be merged: {}",
                        dups.iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                ));
            }
            for sub in &group.items {
                check_tree(sub, prefix, violations, line);
            }
        }
        UseTree::Path(p) => {
            let new_prefix = if prefix.is_empty() {
                p.ident.to_string()
            } else {
                format!("{prefix}::{}", p.ident)
            };
            check_tree(&p.tree, &new_prefix, violations, line);
        }
        _ => {}
    }
}

impl Rule for SortUse {
    fn name(&self) -> &'static str {
        "sort-use"
    }

    fn check(&self, file: &syn::File) -> Vec<(usize, String)> {
        let mut violations = vec![];

        for item in &file.items {
            let Item::Use(u) = item else { continue };
            let line = span_line(u.use_token.span);

            // Check 1: top-level use {} items must be sorted by origin.
            if let UseTree::Group(group) = &u.tree {
                let mut prev: Option<common::Origin> = None;
                for sub in &group.items {
                    let origin = tree_origin(sub);
                    if let Some(p) = prev {
                        if p > origin {
                            violations.push((
                                line,
                                format!(
                                    "items in use {{ }} block are not sorted by origin: {} appears after {}",
                                    common::origin_name(origin),
                                    common::origin_name(p),
                                ),
                            ));
                            break;
                        }
                    }
                    prev = Some(origin);
                }
            }

            // Check 2: recursively check no sibling items share a first segment.
            check_tree(&u.tree, "", &mut violations, line);
        }

        violations
    }
}
