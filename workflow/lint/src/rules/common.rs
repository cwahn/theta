use syn::{ItemUse, UseTree};

/// Import origin classification used across multiple rules.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    Std = 0,
    External = 1,
    Crate = 2,
}

pub fn classify_root(name: &str) -> Origin {
    match name {
        "std" | "core" | "alloc" => Origin::Std,
        "crate" | "self" | "super" => Origin::Crate,
        _ => Origin::External,
    }
}

pub fn use_tree_origin(tree: &UseTree) -> Origin {
    match tree {
        UseTree::Path(p) => classify_root(&p.ident.to_string()),
        UseTree::Name(n) => classify_root(&n.ident.to_string()),
        UseTree::Rename(r) => classify_root(&r.ident.to_string()),
        UseTree::Group(g) => g
            .items
            .iter()
            .map(use_tree_origin)
            .min()
            .unwrap_or(Origin::External),
        UseTree::Glob(_) => Origin::External,
    }
}

pub fn use_item_origin(u: &ItemUse) -> Origin {
    use_tree_origin(&u.tree)
}

pub fn origin_name(o: Origin) -> &'static str {
    match o {
        Origin::Std => "std",
        Origin::External => "external-crate",
        Origin::Crate => "crate/self",
    }
}
