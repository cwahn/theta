use proc_macro2::Span;
use syn::{Attribute, Item, ItemUse};

use super::{
    Rule,
    common::{Origin, origin_name, use_item_origin},
};

pub struct UseGroupOrder;

fn has_cfg(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("cfg"))
}

fn span_line(span: Span) -> usize {
    span.start().line
}

impl Rule for UseGroupOrder {
    fn name(&self) -> &'static str {
        "use-group-order"
    }

    fn check(&self, file: &syn::File) -> Vec<(usize, String)> {
        let mut violations = vec![];

        let unconditional: Vec<&ItemUse> = file
            .items
            .iter()
            .filter_map(|item| {
                if let Item::Use(u) = item {
                    if !has_cfg(&u.attrs) { Some(u) } else { None }
                } else {
                    None
                }
            })
            .collect();

        let mut prev: Option<(&ItemUse, Origin)> = None;

        for u in unconditional {
            let origin = use_item_origin(u);

            if let Some((prev_u, prev_origin)) = prev {
                if prev_origin > origin {
                    violations.push((
                        span_line(u.use_token.span),
                        format!(
                            "{} import must not appear after {} import; order: std → external-crate → crate",
                            origin_name(origin),
                            origin_name(prev_origin),
                        ),
                    ));
                }

                if prev_origin != origin {
                    let prev_last_line = span_line(prev_u.semi_token.span);
                    let curr_first_line = span_line(u.use_token.span);
                    if curr_first_line <= prev_last_line + 1 {
                        violations.push((
                            span_line(u.use_token.span),
                            format!(
                                "missing blank line between {} and {} import groups",
                                origin_name(prev_origin),
                                origin_name(origin),
                            ),
                        ));
                    }
                }
            }

            prev = Some((u, origin));
        }

        violations
    }
}
