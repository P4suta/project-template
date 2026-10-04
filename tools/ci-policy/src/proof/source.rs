use anyhow::{Result, ensure};
use proc_macro2::{TokenStream, TokenTree};
use syn::{Meta, Token, punctuated::Punctuated, visit::Visit};

#[derive(Clone, Copy)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
enum Input {
    InlineModule,
    ExternalModule,
    StandardMacro,
    ExternalMacro,
    StandardAttribute,
    ExternalAttribute,
    MacroDefinition,
    ExternalCrate,
    ForeignModule,
}

fn admitted(input: Input) -> bool {
    match input {
        Input::InlineModule | Input::StandardMacro | Input::StandardAttribute => true,
        Input::ExternalModule
        | Input::ExternalMacro
        | Input::ExternalAttribute
        | Input::MacroDefinition
        | Input::ExternalCrate
        | Input::ForeignModule => false,
    }
}

fn standard_macro(name: &str) -> bool {
    matches!(
        name,
        "assert"
            | "assert_eq"
            | "assert_ne"
            | "debug_assert"
            | "debug_assert_eq"
            | "debug_assert_ne"
            | "matches"
            | "panic"
            | "unreachable"
            | "todo"
            | "format_args"
            | "concat"
            | "stringify"
            | "vec"
            | "cover"
    )
}

fn closed_tokens(tokens: TokenStream) -> bool {
    let mut tokens = tokens.into_iter().peekable();
    while let Some(token) = tokens.next() {
        match token {
            TokenTree::Group(group) if !closed_tokens(group.stream()) => return false,
            TokenTree::Ident(name)
                if macro_invocation(
                    matches!(tokens.peek(), Some(TokenTree::Punct(mark)) if mark.as_char() == '!'),
                    matches!(tokens.clone().nth(1), Some(TokenTree::Group(_))),
                ) && !standard_macro(&name.to_string()) =>
            {
                return false;
            }
            TokenTree::Group(_)
            | TokenTree::Ident(_)
            | TokenTree::Punct(_)
            | TokenTree::Literal(_) => {}
        }
    }
    true
}

fn macro_invocation(bang: bool, arguments: bool) -> bool {
    bang && arguments
}

fn standard_attribute(meta: &Meta) -> bool {
    if let Meta::List(list) = meta
        && !closed_tokens(list.tokens.clone())
    {
        return false;
    }
    let path = meta.path();
    if path.is_ident("cfg_attr") {
        return match meta {
            Meta::List(list) => list
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .is_ok_and(|items| {
                    items.len() >= 2 && items.iter().skip(1).all(standard_attribute)
                }),
            Meta::Path(_) | Meta::NameValue(_) => false,
        };
    }
    let name = path
        .segments
        .iter()
        .map(|part| part.ident.to_string())
        .collect::<Vec<_>>()
        .join("::");
    matches!(
        name.as_str(),
        "cfg"
            | "derive"
            | "inline"
            | "must_use"
            | "expect"
            | "doc"
            | "deprecated"
            | "repr"
            | "kani::proof"
            | "kani::unwind"
    )
}

struct Closure {
    valid: bool,
}

impl Closure {
    fn require(&mut self, input: Input) {
        self.valid &= admitted(input);
    }
}

impl<'ast> Visit<'ast> for Closure {
    fn visit_item_foreign_mod(&mut self, _: &'ast syn::ItemForeignMod) {
        self.require(Input::ForeignModule);
    }

    fn visit_use_rename(&mut self, _: &'ast syn::UseRename) {
        self.require(Input::ExternalMacro);
    }

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        self.require(if item.content.is_some() {
            Input::InlineModule
        } else {
            Input::ExternalModule
        });
        syn::visit::visit_item_mod(self, item);
    }

    fn visit_item_extern_crate(&mut self, _: &'ast syn::ItemExternCrate) {
        self.require(Input::ExternalCrate);
    }

    fn visit_item_macro(&mut self, item: &'ast syn::ItemMacro) {
        if item.ident.is_some() {
            self.require(Input::MacroDefinition);
        }
        syn::visit::visit_item_macro(self, item);
    }

    fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
        self.require(if standard_attribute(&attribute.meta) {
            Input::StandardAttribute
        } else {
            Input::ExternalAttribute
        });
        syn::visit::visit_attribute(self, attribute);
    }

    fn visit_macro(&mut self, value: &'ast syn::Macro) {
        let name = value
            .path
            .segments
            .iter()
            .map(|part| part.ident.to_string())
            .collect::<Vec<_>>()
            .join("::");
        let standard = value.path.is_ident(&name) && standard_macro(&name)
            || name.strip_prefix("std::").is_some_and(standard_macro)
            || name == "kani::cover";
        self.require(if standard && closed_tokens(value.tokens.clone()) {
            Input::StandardMacro
        } else {
            Input::ExternalMacro
        });
    }
}

pub fn validate_source(bytes: &[u8]) -> Result<()> {
    let syntax = syn::parse_file(std::str::from_utf8(bytes)?)?;
    let mut closure = Closure { valid: true };
    closure.visit_file(&syntax);
    ensure!(
        closure.valid,
        "standalone proof source contains an external or unbound input"
    );
    Ok(())
}

#[cfg(kani)]
mod proofs {
    use super::{Input, admitted, macro_invocation};

    #[kani::proof]
    fn macro_recognition_requires_bang_and_delimited_arguments() {
        let bang: bool = kani::any();
        let arguments: bool = kani::any();
        let invocation = macro_invocation(bang, arguments);
        assert_eq!(invocation, bang && arguments);
        kani::cover!(invocation);
        kani::cover!(!invocation && bang && !arguments);
        kani::cover!(!invocation && !bang);
    }

    #[kani::proof]
    fn only_closed_source_inputs_are_admitted() {
        let input: Input = kani::any();
        assert_eq!(
            admitted(input),
            matches!(
                input,
                Input::InlineModule | Input::StandardMacro | Input::StandardAttribute
            )
        );
        kani::cover!(admitted(input));
        kani::cover!(!admitted(input));
    }
}
