use syn::{ItemUse, visit::Visit};

use super::Rule;

pub struct UseToplevel;

struct InlineUseVisitor {
    violations: Vec<(usize, String)>,
    in_block: bool,
}

impl Visit<'_> for InlineUseVisitor {
    fn visit_block(&mut self, block: &syn::Block) {
        let prev = self.in_block;
        self.in_block = true;
        syn::visit::visit_block(self, block);
        self.in_block = prev;
    }

    // Nested mod items have their own module scope — do not treat as a block.
    fn visit_item_mod(&mut self, m: &syn::ItemMod) {
        syn::visit::visit_item_mod(self, m);
    }

    fn visit_item_use(&mut self, u: &ItemUse) {
        if self.in_block {
            let line = u.use_token.span.start().line;
            violations_push(
                &mut self.violations,
                line,
                "use item inside a block; all use statements must be at module level",
            );
        }
        // Do not recurse — use items have no children to visit.
    }
}

fn violations_push(v: &mut Vec<(usize, String)>, line: usize, msg: &str) {
    v.push((line, msg.into()));
}

impl Rule for UseToplevel {
    fn name(&self) -> &'static str {
        "use-toplevel"
    }

    fn check(&self, file: &syn::File) -> Vec<(usize, String)> {
        let mut visitor = InlineUseVisitor {
            violations: vec![],
            in_block: false,
        };
        syn::visit::visit_file(&mut visitor, file);
        visitor.violations
    }
}
