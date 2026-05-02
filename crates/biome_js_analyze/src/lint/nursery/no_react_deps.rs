use crate::{JsRuleAction, services::semantic::Semantic};
use biome_analyze::{
    FixKind, Rule, RuleDiagnostic, RuleDomain, RuleSource, context::RuleContext, declare_lint_rule,
};
use biome_console::markup;
use biome_diagnostics::Severity;
use biome_js_factory::make;
use biome_js_semantic::SemanticModel;
use biome_js_syntax::{
    AnyJsCallArgument, AnyJsExpression, JsCallArgumentList, JsCallExpression, JsSyntaxKind,
    JsVariableDeclarator, binding_ext::AnyJsIdentifierBinding, global_identifier,
};
use biome_rowan::{AstNode, AstSeparatedList, BatchMutationExt, TextRange};
use biome_rule_options::no_react_deps::NoReactDepsOptions;

declare_lint_rule! {
    /// Disallow usage of dependency arrays in `createEffect` and `createMemo`.
    ///
    /// Solid's reactivity system tracks dependencies automatically. Passing a
    /// dependency array as a second argument is a React idiom that has no effect
    /// in Solid: `createEffect` and `createMemo` use the second argument as the
    /// initial value passed to the function on its first run, not as a list of
    /// dependencies.
    ///
    /// ## Examples
    ///
    /// ### Invalid
    ///
    /// ```js,expect_diagnostic
    /// import { createEffect } from "solid-js";
    /// createEffect(() => {
    ///   console.log(signal());
    /// }, [signal()]);
    /// ```
    ///
    /// ```js,expect_diagnostic
    /// import { createMemo } from "solid-js";
    /// const value = createMemo(() => computeExpensiveValue(a(), b()), [a, b]);
    /// ```
    ///
    /// ### Valid
    ///
    /// ```js
    /// import { createEffect, createMemo } from "solid-js";
    /// createEffect(() => {
    ///   console.log(signal());
    /// });
    ///
    /// createEffect((prev) => {
    ///   console.log(signal());
    ///   return prev + 1;
    /// }, 0);
    ///
    /// const value = createMemo(() => computeExpensiveValue(a(), b()));
    /// const sum = createMemo((prev) => input() + prev, 0);
    /// ```
    ///
    pub NoReactDeps {
        version: "next",
        name: "noReactDeps",
        language: "js",
        sources: &[RuleSource::EslintSolid("no-react-deps").same()],
        recommended: true,
        severity: Severity::Warning,
        fix_kind: FixKind::Unsafe,
        domains: &[RuleDomain::Solid],
    }
}

pub struct RuleState {
    /// Range of the offending dependency-array argument (used for the diagnostic).
    deps_range: TextRange,
    /// Name of the called function (`createEffect` or `createMemo`).
    callee_name: &'static str,
    /// `true` when the dependency argument is an inline array literal that
    /// can safely be removed by the auto-fix.
    is_inline_array: bool,
}

impl Rule for NoReactDeps {
    type Query = Semantic<JsCallExpression>;
    type State = RuleState;
    type Signals = Option<Self::State>;
    type Options = NoReactDepsOptions;

    fn run(ctx: &RuleContext<Self>) -> Self::Signals {
        let call = ctx.query();
        let model = ctx.model();

        let callee_name = matched_callee_name(call, model)?;

        let arguments = call.arguments().ok()?;
        let mut args_iter = arguments.args().iter();

        let first = args_iter.next()?.ok()?;
        // Skip spread or anything that isn't a regular expression argument.
        if first.as_js_spread().is_some() {
            return None;
        }

        let second = args_iter.next()?.ok()?;
        let AnyJsCallArgument::AnyJsExpression(deps_expr) = second else {
            return None;
        };

        let is_inline_array = matches!(
            deps_expr.clone().omit_parentheses(),
            AnyJsExpression::JsArrayExpression(_)
        );
        let resolves_to_array = is_inline_array
            || identifier_resolves_to_array_literal(&deps_expr, model).unwrap_or(false);

        if !resolves_to_array {
            return None;
        }

        Some(RuleState {
            deps_range: deps_expr.range(),
            callee_name,
            is_inline_array,
        })
    }

    fn diagnostic(_ctx: &RuleContext<Self>, state: &Self::State) -> Option<RuleDiagnostic> {
        let callee_name = state.callee_name;
        Some(
            RuleDiagnostic::new(
                rule_category!(),
                state.deps_range,
                markup! {
                    "Dependency arrays have no effect in "<Emphasis>{callee_name}</Emphasis>"."
                },
            )
            .note(markup! {
                "Solid tracks reactive dependencies automatically. The second argument is used as the initial value passed to the function, not as a dependency list."
            }),
        )
    }

    fn action(ctx: &RuleContext<Self>, state: &Self::State) -> Option<JsRuleAction> {
        if !state.is_inline_array {
            return None;
        }

        let call = ctx.query();
        let arguments = call.arguments().ok()?;
        let argument_list = arguments.args();

        let mut args_iter = argument_list.iter();
        let first = args_iter.next()?.ok()?;

        let new_args = make::js_call_argument_list([first], []);

        let mut mutation = ctx.root().begin();
        mutation.replace_node::<JsCallArgumentList>(argument_list, new_args);

        Some(JsRuleAction::new(
            ctx.metadata().action_category(ctx.category(), ctx.group()),
            ctx.metadata().applicability(),
            markup! { "Remove the dependency array." }.to_owned(),
            mutation,
        ))
    }
}

/// Returns the name of the call's callee if it matches `createEffect` or
/// `createMemo` and the identifier is not shadowed by a non-import local
/// binding.
fn matched_callee_name(call: &JsCallExpression, model: &SemanticModel) -> Option<&'static str> {
    let callee = call.callee().ok()?.omit_parentheses();
    let (reference, name) = global_identifier(&callee)?;

    let matched = match name.text() {
        "createEffect" => "createEffect",
        "createMemo" => "createMemo",
        _ => return None,
    };

    match model.binding(&reference) {
        // Not bound: assume it is the global / auto-imported Solid helper.
        None => Some(matched),
        Some(binding) => {
            if is_imported_binding(&binding.tree()) {
                Some(matched)
            } else {
                None
            }
        }
    }
}

fn is_imported_binding(binding: &AnyJsIdentifierBinding) -> bool {
    binding
        .syntax()
        .ancestors()
        .any(|ancestor: biome_js_syntax::JsSyntaxNode| {
            matches!(
                ancestor.kind(),
                JsSyntaxKind::JS_IMPORT_NAMED_CLAUSE
                    | JsSyntaxKind::JS_IMPORT_DEFAULT_CLAUSE
                    | JsSyntaxKind::JS_IMPORT_NAMESPACE_CLAUSE
                    | JsSyntaxKind::JS_NAMED_IMPORT_SPECIFIER
                    | JsSyntaxKind::JS_SHORTHAND_NAMED_IMPORT_SPECIFIER
                    | JsSyntaxKind::JS_DEFAULT_IMPORT_SPECIFIER
                    | JsSyntaxKind::JS_NAMESPACE_IMPORT_SPECIFIER
            )
        })
}

/// Resolve `expr` to a variable initializer and return `true` when that
/// initializer is an array literal.
fn identifier_resolves_to_array_literal(
    expr: &AnyJsExpression,
    model: &SemanticModel,
) -> Option<bool> {
    let identifier = expr.as_js_identifier_expression()?;
    let reference = identifier.name().ok()?;
    let binding = model.binding(&reference)?;
    let declarator = binding
        .tree()
        .syntax()
        .ancestors()
        .find_map(JsVariableDeclarator::cast)?;
    let initializer = declarator.initializer()?.expression().ok()?;
    Some(matches!(
        initializer.omit_parentheses(),
        AnyJsExpression::JsArrayExpression(_)
    ))
}
