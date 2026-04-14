use proc_macro2::Span;
use syn::{Attribute, Item, Meta};

use super::{Rule, common::{Origin, use_item_origin}};

pub struct CfgOrder;

fn cfg_attr(attrs: &[Attribute]) -> Option<&Attribute> {
    attrs.iter().find(|a| a.path().is_ident("cfg"))
}

/// Rough complexity: number of commas in the cfg token stream + 1.
fn cfg_complexity(attr: &Attribute) -> usize {
    if let Meta::List(list) = &attr.meta {
        list.tokens
            .to_string()
            .chars()
            .filter(|&c| c == ',')
            .count()
            + 1
    } else {
        1
    }
}

fn cfg_string(attr: &Attribute) -> String {
    if let Meta::List(list) = &attr.meta {
        list.tokens.to_string()
    } else {
        String::new()
    }
}

fn span_line(span: Span) -> usize {
    span.start().line
}

impl Rule for CfgOrder {
    fn name(&self) -> &'static str {
        "cfg-order"
    }

    fn check(&self, file: &syn::File) -> Vec<(usize, String)> {
        let mut violations = vec![];
        // (complexity, cfg_string, origin)
        let mut last_cfg: Option<(usize, String, Origin)> = None;
        let mut seen_any_cfg = false;

        for item in &file.items {
            let Item::Use(u) = item else { continue };

            match cfg_attr(&u.attrs) {
                None => {
                    if seen_any_cfg {
                        let line = span_line(u.use_token.span);
                        violations.push((
                            line,
                            "unconditional use item must come before all cfg-gated use items"
                                .into(),
                        ));
                    }
                }
                Some(attr) => {
                    seen_any_cfg = true;
                    let complexity = cfg_complexity(attr);
                    let s = cfg_string(attr);
                    let origin = use_item_origin(u);
                    let current = (complexity, s.clone(), origin);

                    if let Some((pc, ref ps, po)) = last_cfg {
                        if (pc, ps.as_str(), po) > (complexity, s.as_str(), origin) {
                            let line = span_line(u.use_token.span);
                            violations.push((
                                line,
                                format!(
                                    "cfg-gated use out of order: current cfg({s}) should not precede cfg({ps})"
                                ),
                            ));
                        }
                    }
                    last_cfg = Some(current);
                }
            }
        }

        violations
    }
}
