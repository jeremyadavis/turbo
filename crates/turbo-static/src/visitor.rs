//! A visitor that traverses the AST and collects all functions or methods that
//! are annotated with `#[turbo_tasks::function]`.

use std::collections::HashMap;

use syn::{visit::Visit, Meta};

pub struct TaskVisitor {
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
            (first.ident == "turbo_tasks" && second.ident == "function").then(|| vec![])
        }
        Meta::List(list) if list.path.segments.len() == 2 => {
            let first = &list.path.segments[0];
            let second = &list.path.segments[1];
            if (first.ident != "turbo_tasks" || second.ident != "function") {
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
