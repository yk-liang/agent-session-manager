//! Tree-sitter JavaScript 语法与词法作用域辅助函数。
use std::{borrow::Cow, collections::HashSet};
use tree_sitter::{Node, Parser, Tree};

pub(super) const MAX_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_TOOL_NAME_BYTES: usize = 128;

pub(super) fn parse(script: &str) -> Option<Tree> {
    if script.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_javascript::LANGUAGE.into()).ok()?;
    parser.parse(script, None)
}

pub(super) fn children(node: Node<'_>) -> impl DoubleEndedIterator<Item = Node<'_>> {
    (0..node.named_child_count()).filter_map(move |index| node.named_child(index as u32))
        .filter(|node| !node.is_extra())
}

pub(super) fn only_child(node: Node<'_>) -> Option<Node<'_>> {
    let mut nodes = children(node);
    let first = nodes.next()?;
    nodes.next().is_none().then_some(first)
}

pub(super) fn source<'s>(node: Node<'_>, script: &'s str) -> &'s str {
    &script[node.byte_range()]
}

pub(super) fn unparen(mut node: Node<'_>) -> Node<'_> {
    while node.kind() == "parenthesized_expression" {
        let Some(inner) = only_child(node) else { break };
        node = inner;
    }
    node
}

pub(super) fn is_field(parent: Node<'_>, field: &str, child: Node<'_>) -> bool {
    parent.child_by_field_name(field) == Some(child)
}

/// 解析 `<obj>.prop` 或 `<obj>["prop"]`，返回对象标识符和属性名。
pub(super) fn member_call<'s>(node: Node<'_>, script: &'s str) -> Option<(&'s str, Cow<'s, str>)> {
    let node = unparen(node);
    let object = unparen(node.child_by_field_name("object")?);
    if object.kind() != "identifier" {
        return None;
    }
    let name = match node.kind() {
        "member_expression" => Cow::Borrowed(source(node.child_by_field_name("property")?, script)),
        "subscript_expression" => Cow::Owned(literal_string(node.child_by_field_name("index")?, script)?),
        _ => return None,
    };
    (name.len() <= MAX_TOOL_NAME_BYTES).then_some((source(object, script), name))
}

pub(super) fn arguments(node: Node<'_>) -> Option<Node<'_>> {
    let arguments = node.child_by_field_name("arguments")?;
    (arguments.kind() == "arguments").then_some(arguments)
}

/// 字符串字面量的转义；`raw` 是 `String.raw` 模板，只统一换行。
pub(super) fn unescape(content: &str, raw: bool) -> Option<String> {
    if raw {
        return Some(content.replace("\r\n", "\n").replace('\r', "\n"));
    }
    if !content.contains('\\') {
        return Some(content.replace("\r\n", "\n"));
    }
    let mut output = Vec::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    while let Some(ch) = chars.next() {
        let char = if ch != '\\' { ch } else {
            match chars.next()? {
                '\n' | '\u{2028}' | '\u{2029}' => continue,
                '\r' => { chars.next_if_eq(&'\n'); continue; }
                'n' => '\n', 'r' => '\r', 't' => '\t',
                'b' => '\u{0008}', 'f' => '\u{000c}', 'v' => '\u{000b}',
                '0' if !chars.peek().is_some_and(char::is_ascii_digit) => '\0',
                '0'..='9' => return None,
                'u' if chars.next_if_eq(&'{').is_some() => {
                    let mut scalar = 0u32;
                    loop {
                        let ch = chars.next()?;
                        if ch == '}' { break; }
                        scalar = scalar.checked_mul(16)?.checked_add(ch.to_digit(16)?)?;
                    }
                    char::from_u32(scalar)?
                }
                mode @ ('x' | 'u') => {
                    let value = (0..if mode == 'x' { 2 } else { 4 })
                        .try_fold(0u16, |value, _| Some(value * 16 + chars.next()?.to_digit(16)? as u16))?;
                    output.push(value);
                    continue;
                }
                other => other,
            }
        };
        output.extend_from_slice(char.encode_utf16(&mut [0; 2]));
    }
    String::from_utf16(&output).ok()
}

/// 模板字符串的引号内部；`raw` 表示 `String.raw` 标签。
pub(super) fn template<'t>(node: Node<'t>, script: &str) -> Option<(Node<'t>, bool)> {
    let node = unparen(node);
    match node.kind() {
        "template_string" => Some((node, false)),
        "call_expression" if source(node.child_by_field_name("function")?, script) == "String.raw" => {
            let template = node.child_by_field_name("arguments")?;
            (template.kind() == "template_string").then_some((template, true))
        }
        _ => None,
    }
}

fn quoted(node: Node<'_>, script: &str) -> Option<String> {
    let text = source(node, script);
    text.get(1..text.len().checked_sub(1)?).map(Into::into)
}

pub(super) fn literal_string(node: Node<'_>, script: &str) -> Option<String> {
    let node = unparen(node);
    if node.kind() == "string" {
        return unescape(&quoted(node, script)?, false);
    }
    let (template, raw) = template(node, script)?;
    if children(template).any(|child| child.kind() == "template_substitution") {
        return None;
    }
    unescape(&quoted(template, script)?, raw)
}

pub(super) fn key_name<'s>(key: Node<'_>, script: &'s str) -> Option<Cow<'s, str>> {
    match key.kind() {
        "property_identifier" | "shorthand_property_identifier" => Some(Cow::Borrowed(source(key, script))),
        "string" => literal_string(key, script).map(Cow::Owned),
        "computed_property_name" => literal_string(only_child(key)?, script).map(Cow::Owned),
        "number" => Some(Cow::Borrowed(source(key, script))),
        _ => None,
    }
}

fn is_function(kind: &str) -> bool {
    matches!(kind, "arrow_function" | "function_expression" | "function_declaration" | "method_definition" | "generator_function" | "generator_function_declaration")
}

pub(super) fn pattern_names<'s>(node: Node<'_>, script: &'s str, out: &mut Vec<&'s str>) {
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        match node.kind() {
            "identifier" | "shorthand_property_identifier_pattern" => out.push(source(node, script)),
            "pair_pattern" => pending.extend(node.child_by_field_name("value")),
            "assignment_pattern" | "object_assignment_pattern" => pending.extend(node.child_by_field_name("left")),
            "array_pattern" | "object_pattern" | "rest_pattern" | "formal_parameters" => pending.extend(children(node)),
            _ => {}
        }
    }
}

fn declarator_names<'s>(declaration: Node<'_>, script: &'s str, out: &mut Vec<&'s str>) {
    for child in children(declaration) {
        if child.kind() == "variable_declarator" {
            if let Some(name) = child.child_by_field_name("name") {
                pattern_names(name, script, out);
            }
        }
    }
}

/// 收集函数体内的 `var` 和嵌套 `function` 声明（不跨入更内层函数）。
pub(super) fn hoisted_names<'s>(root: Node<'_>, script: &'s str, names: &mut Vec<&'s str>) {
    let mut pending: Vec<_> = children(root).collect();
    while let Some(child) = pending.pop() {
        if is_function(child.kind()) {
            if matches!(child.kind(), "function_declaration" | "generator_function_declaration") {
                if let Some(name) = child.child_by_field_name("name") {
                    names.push(source(name, script));
                }
            }
            continue;
        }
        if child.kind() == "variable_declaration" {
            declarator_names(child, script, names);
        }
        pending.extend(children(child));
    }
}

/// 词法作用域入口处先登记该作用域声明的所有名字（`Unknown`），遮蔽外层同名绑定并处理提升与 TDZ。
pub(super) fn scope_names<'s>(node: Node<'_>, script: &'s str) -> Vec<&'s str> {
    let mut names = Vec::new();
    let collect_statements = |container: Node<'_>, names: &mut Vec<&'s str>| {
        for child in children(container) {
            match child.kind() {
                "lexical_declaration" => declarator_names(child, script, names),
                "function_declaration" | "generator_function_declaration" | "class_declaration" => {
                    if let Some(name) = child.child_by_field_name("name") {
                        names.push(source(name, script));
                    }
                }
                _ => {}
            }
        }
    };
    match node.kind() {
        "program" => {
            collect_statements(node, &mut names);
            hoisted_names(node, script, &mut names);
        }
        "statement_block" => collect_statements(node, &mut names),
        "switch_body" => {
            for clause in children(node) {
                collect_statements(clause, &mut names);
            }
        }
        "for_in_statement" => {
            if let Some(left) = node.child_by_field_name("left") {
                pattern_names(left, script, &mut names);
            }
        }
        "for_statement" => {
            if let Some(init) = node.child_by_field_name("initializer").filter(|init| init.kind() == "lexical_declaration") {
                declarator_names(init, script, &mut names);
            }
        }
        "catch_clause" => {
            if let Some(parameter) = node.child_by_field_name("parameter") {
                pattern_names(parameter, script, &mut names);
            }
        }
        kind if is_function(kind) => {
            if let Some(name) = node.child_by_field_name("name").filter(|name| name.kind() == "identifier") {
                names.push(source(name, script));
            }
            if let Some(parameter) = node.child_by_field_name("parameter") {
                pattern_names(parameter, script, &mut names);
            }
            if let Some(parameters) = node.child_by_field_name("parameters") {
                pattern_names(parameters, script, &mut names);
            }
            if let Some(body) = node.child_by_field_name("body") {
                hoisted_names(body, script, &mut names);
            }
        }
        _ => {}
    }
    names
}

pub(super) fn creates_scope(kind: &str) -> bool {
    matches!(kind, "program" | "statement_block" | "switch_body" | "for_in_statement" | "for_statement" | "catch_clause")
        || is_function(kind)
}

pub(super) fn root_identifier<'s>(mut node: Node<'_>, script: &'s str) -> Option<&'s str> {
    loop {
        node = unparen(node);
        match node.kind() {
            "identifier" => return Some(source(node, script)),
            "member_expression" | "subscript_expression" => node = node.child_by_field_name("object")?,
            _ => return None,
        }
    }
}

pub(super) fn mutating_call_target<'s>(node: Node<'_>, script: &'s str) -> Option<&'s str> {
    let function = node.child_by_field_name("function")?;
    if function.kind() == "member_expression" {
        let method = function.child_by_field_name("property").map(|property| source(property, script));
        if matches!(method, Some("push" | "pop" | "shift" | "unshift" | "splice" | "sort" | "reverse" | "fill" | "copyWithin")) {
            return function.child_by_field_name("object").and_then(|object| root_identifier(object, script));
        }
    }
    if source(function, script) == "Object.assign" {
        return arguments(node).and_then(|args| children(args).next()).and_then(|target| root_identifier(target, script));
    }
    None
}

/// 循环体内被重新赋值或原地修改的变量名：命中循环变量时放弃展开。
pub(super) fn body_written_names<'s>(root: Node<'_>, script: &'s str) -> HashSet<&'s str> {
    let mut written = HashSet::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        match node.kind() {
            "assignment_expression" | "augmented_assignment_expression" => {
                if let Some(left) = node.child_by_field_name("left").map(unparen) {
                    if matches!(left.kind(), "member_expression" | "subscript_expression") {
                        written.extend(root_identifier(left, script));
                    } else {
                        let mut names = Vec::new();
                        pattern_names(left, script, &mut names);
                        written.extend(names);
                    }
                }
            }
            "update_expression" => {
                if let Some(arg) = node.child_by_field_name("argument") {
                    written.extend(root_identifier(arg, script));
                }
            }
            "unary_expression" if node.child_by_field_name("operator").is_some_and(|op| source(op, script) == "delete") => {
                if let Some(arg) = node.child_by_field_name("argument") {
                    written.extend(root_identifier(arg, script));
                }
            }
            "call_expression" => written.extend(mutating_call_target(node, script)),
            "for_in_statement" => {
                if let Some(left) = node.child_by_field_name("left") {
                    let mut names = Vec::new();
                    pattern_names(left, script, &mut names);
                    written.extend(names);
                }
            }
            _ => {}
        }
        pending.extend(children(node));
    }
    written
}
