//! A visitor that traverses the AST and collects all functions or methods that
//! are annotated with `#[turbo_tasks::function]`.

use std::cmp::Ordering;

use syn::{spanned::Spanned, visit::Visit, Expr, Meta};

pub struct TaskVisitor {
    /// the list of results as pairs of an identifier and its tags
    pub results: Vec<(syn::Ident, Vec<String>)>,
}

impl TaskVisitor {
    pub fn new() -> Self {
        Self {
            results: Default::default(),
        }
    }
}

impl Visit<'_> for TaskVisitor {
    #[tracing::instrument(skip_all)]
    fn visit_item_fn(&mut self, i: &syn::ItemFn) {
        if let Some(tags) = extract_tags(i.attrs.iter()) {
            tracing::trace!("L{}: {}", i.sig.ident.span().start().line, i.sig.ident,);
            self.results.push((i.sig.ident.clone(), tags));
        }
    }

    #[tracing::instrument(skip_all)]
    fn visit_impl_item_fn(&mut self, i: &syn::ImplItemFn) {
        if let Some(tags) = extract_tags(i.attrs.iter()) {
            tracing::trace!("L{}: {}", i.sig.ident.span().start().line, i.sig.ident,);
            self.results.push((i.sig.ident.clone(), tags));
        }
    }
}

fn extract_tags<'a>(mut meta: impl Iterator<Item = &'a syn::Attribute>) -> Option<Vec<String>> {
    meta.find_map(|a| match &a.meta {
        // path has two segments, turbo_tasks and function
        Meta::Path(path) if path.segments.len() == 2 => {
            let first = &path.segments[0];
            let second = &path.segments[1];
            (first.ident == "turbo_tasks" && second.ident == "function").then(std::vec::Vec::new)
        }
        Meta::List(list) if list.path.segments.len() == 2 => {
            let first = &list.path.segments[0];
            let second = &list.path.segments[1];
            if first.ident != "turbo_tasks" || second.ident != "function" {
                return None;
            }

            // collect ident tokens as args
            let tags: Vec<_> = list
                .tokens
                .clone()
                .into_iter()
                .filter_map(|t| {
                    if let proc_macro2::TokenTree::Ident(ident) = t {
                        Some(ident.to_string())
                    } else {
                        None
                    }
                })
                .collect();

            Some(tags)
        }
        _ => {
            tracing::trace!("skipping unknown annotation");
            None
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallingStyle {
    Once,
    #[allow(dead_code)]
    ZeroOrOnce,
    #[allow(dead_code)]
    ZeroOrMore,
    #[allow(dead_code)]
    OneOrMore,
}

impl PartialOrd for CallingStyle {
    /// Acts like boolean addition over zero, once, more than once. Zero <
    /// ZeroOrOnce since it has more possibilities.
    ///
    /// This is partial because comparing `ZeroOrOnce` and `OneOrMore` is
    /// meaningless.
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        use CallingStyle::*;
        match (self, other) {
            (Once, Once)
            | (ZeroOrOnce, ZeroOrOnce)
            | (OneOrMore, OneOrMore)
            | (ZeroOrMore, ZeroOrMore) => Some(Ordering::Equal),
            (ZeroOrOnce, OneOrMore) | (OneOrMore, ZeroOrOnce) => None,
            (Once, ZeroOrOnce) => Some(Ordering::Greater),
            (Once, ZeroOrMore) => Some(Ordering::Greater),
            (Once, OneOrMore) => Some(Ordering::Greater),
            (ZeroOrOnce, Once) => Some(Ordering::Less),
            (ZeroOrOnce, ZeroOrMore) => Some(Ordering::Greater),
            (ZeroOrMore, Once) => Some(Ordering::Less),
            (ZeroOrMore, ZeroOrOnce) => Some(Ordering::Less),
            (ZeroOrMore, OneOrMore) => Some(Ordering::Less),
            (OneOrMore, Once) => Some(Ordering::Less),
            (OneOrMore, ZeroOrMore) => Some(Ordering::Greater),
        }
    }
}

pub struct CallingStyleVisitor {
    pub reference: crate::IdentifierReference,
    pub call_type: Option<CallingStyle>,

    state: Option<CallingStyleVisitorState>,
}

impl CallingStyleVisitor {
    /// Create a new visitor that will traverse the AST and determine the
    /// calling style of the target function within the source function.
    pub fn new(reference: crate::IdentifierReference) -> Self {
        Self {
            reference,
            call_type: None,
            state: None,
        }
    }

    pub fn result(self) -> Option<CallingStyle> {
        self.call_type
    }
}

#[derive(Debug, Clone, Copy)]
enum CallingStyleVisitorState {
    Block,
    Loop,
    If,
    Closure,
}

impl Visit<'_> for CallingStyleVisitor {
    fn visit_item_fn(&mut self, i: &'_ syn::ItemFn) {
        if self.reference.identifier.equals_ident(&i.sig.ident, true) {
            self.call_type = Some(CallingStyle::Once);
            self.state = Some(CallingStyleVisitorState::Block);
            syn::visit::visit_item_fn(self, i);
            self.state = None;
        }
    }

    fn visit_impl_item_fn(&mut self, i: &'_ syn::ImplItemFn) {
        if self.reference.identifier.equals_ident(&i.sig.ident, true) {
            self.call_type = Some(CallingStyle::Once);
            self.state = Some(CallingStyleVisitorState::Block);
            syn::visit::visit_impl_item_fn(self, i);
            self.state = None;
        }
    }

    fn visit_expr_loop(&mut self, i: &'_ syn::ExprLoop) {
        let state = self.state;
        self.state = Some(CallingStyleVisitorState::Loop);
        syn::visit::visit_expr_loop(self, i);
        self.state = state;
    }

    fn visit_expr_for_loop(&mut self, i: &'_ syn::ExprForLoop) {
        let state = self.state;
        self.state = Some(CallingStyleVisitorState::Loop);
        syn::visit::visit_expr_for_loop(self, i);
        self.state = state;
    }

    fn visit_expr_if(&mut self, i: &'_ syn::ExprIf) {
        let state = self.state;
        self.state = Some(CallingStyleVisitorState::If);
        syn::visit::visit_expr_if(self, i);
        self.state = state;
    }

    fn visit_expr_closure(&mut self, i: &'_ syn::ExprClosure) {
        let state = self.state;
        self.state = Some(CallingStyleVisitorState::Closure);
        syn::visit::visit_expr_closure(self, i);
        self.state = state;
    }

    fn visit_expr_call(&mut self, i: &'_ syn::ExprCall) {
        match i.func.as_ref() {
            Expr::Path(p) => {
                println!("{:?} - {:?}", p.span(), self.reference.references)
            }
            rest => {
                tracing::info!("visiting call: {:?}", rest);
            }
        }
    }
}
