//! Static descriptions of Codex's freeform JavaScript cells.
use super::super::tool_ops::{self, line_count, ExecCall, ExecFeature, Op};
use serde_json::Value;
use std::{borrow::Cow, collections::HashMap};
use tree_sitter::{Node, Parser, Tree};

const MAX_SOURCE_BYTES: usize = 1024 * 1024;
const WEB_ACTIONS: [&str; 11] = ["search_query", "open", "click", "find", "image_query", "screenshot", "weather",
    "finance", "sports", "time", "literature_search_query"];

#[derive(PartialEq, Eq)]
pub(super) enum ExecutionKey {
    Command(String),
    Patch(Vec<String>),
    Image(String),
    Mcp(String, String),
    Web(&'static str),
    Sleep,
}

fn parse(script: &str) -> Option<Tree> {
    if script.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_javascript::LANGUAGE.into()).ok()?;
    parser.parse(script, None)
}

fn children(node: Node<'_>) -> impl DoubleEndedIterator<Item = Node<'_>> {
    (0..node.named_child_count()).filter_map(move |index| node.named_child(index as u32))
        .filter(|node| !node.is_extra())
}

fn only_child(node: Node<'_>) -> Option<Node<'_>> {
    let mut nodes = children(node);
    let first = nodes.next()?;
    nodes.next().is_none().then_some(first)
}

fn source<'s>(node: Node<'_>, script: &'s str) -> &'s str {
    &script[node.byte_range()]
}

fn unparen(mut node: Node<'_>) -> Node<'_> {
    while node.kind() == "parenthesized_expression" {
        let Some(inner) = only_child(node) else { break };
        node = inner;
    }
    node
}

fn tool_name<'s>(node: Node<'_>, script: &'s str) -> Option<Cow<'s, str>> {
    let node = unparen(node);
    let object = node.child_by_field_name("object")?;
    if object.kind() != "identifier" || source(object, script) != "tools" {
        return None;
    }
    match node.kind() {
        "member_expression" => Some(Cow::Borrowed(source(node.child_by_field_name("property")?, script))),
        "subscript_expression" => literal_string(node.child_by_field_name("index")?, script).map(Cow::Owned),
        _ => None,
    }
}

fn arguments(node: Node<'_>) -> Option<Node<'_>> {
    let arguments = node.child_by_field_name("arguments")?;
    (arguments.kind() == "arguments").then_some(arguments)
}

fn constant_binding(node: Node<'_>) -> Option<(Node<'_>, Node<'_>)> {
    let declaration = node.parent()?;
    let kind = declaration.child_by_field_name("kind")?;
    if kind.kind() != "const" {
        return None;
    }
    let name = node.child_by_field_name("name")?;
    (name.kind() == "identifier").then_some((name, node.child_by_field_name("value")?))
}

#[derive(Default)]
struct Scope<'a> {
    constants: HashMap<&'a str, Value>,
    aliases: HashMap<&'a str, Cow<'a, str>>,
}

impl Scope<'_> {
    fn value(&self, node: Node<'_>, script: &str) -> Option<Value> {
        self.value_at(node, script, 0)
    }

    fn value_at(&self, node: Node<'_>, script: &str, depth: usize) -> Option<Value> {
        if depth > 16 {
            return None;
        }
        let node = unparen(node);
        match node.kind() {
            "identifier" | "shorthand_property_identifier" => self.constants.get(source(node, script)).cloned(),
            "number" => serde_json::from_str(&source(node, script).replace('_', "")).ok(),
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            "null" => Some(Value::Null),
            "binary_expression" if node.child_by_field_name("operator")?.kind() == "+" => {
                let left = self.value_at(node.child_by_field_name("left")?, script, depth + 1)?;
                let right = self.value_at(node.child_by_field_name("right")?, script, depth + 1)?;
                let (left, right) = (left.as_str()?, right.as_str()?);
                (left.len() + right.len() <= MAX_SOURCE_BYTES).then(|| Value::String(format!("{left}{right}")))
            }
            _ => literal_string(node, script).map(Value::String),
        }
    }
}

fn literal_string(node: Node<'_>, script: &str) -> Option<String> {
    let node = unparen(node);
    let (node, raw) = match node.kind() {
        "string" | "template_string" => (node, false),
        "call_expression" if source(node.child_by_field_name("function")?, script) == "String.raw" => {
            let template = node.child_by_field_name("arguments")?;
            (template.kind() == "template_string").then_some((template, true))?
        }
        _ => return None,
    };
    if node.kind() == "template_string" && children(node).any(|child| child.kind() == "template_substitution") {
        return None;
    }
    let text = source(node, script);
    let content = text.get(1..text.len().checked_sub(1)?)?;
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

fn key_name<'s>(key: Node<'_>, script: &'s str) -> Option<Cow<'s, str>> {
    match key.kind() {
        "property_identifier" | "shorthand_property_identifier" => Some(Cow::Borrowed(source(key, script))),
        "string" => literal_string(key, script).map(Cow::Owned),
        "computed_property_name" => literal_string(only_child(key)?, script).map(Cow::Owned),
        "number" => Some(Cow::Borrowed(source(key, script))),
        _ => None,
    }
}

fn field<'a>(object: Node<'a>, key: &str, script: &str) -> Option<Node<'a>> {
    let object = unparen(object);
    if object.kind() != "object" {
        return None;
    }
    for property in children(object).rev() {
        match property.kind() {
            "pair" => {
                if key_name(property.child_by_field_name("key")?, script)? == key {
                    return property.child_by_field_name("value");
                }
            }
            "shorthand_property_identifier" if source(property, script) == key => return Some(property),
            "shorthand_property_identifier" => {}
            _ => return None,
        }
    }
    None
}

fn decode_call(name: &str, call: Node<'_>, script: &str, scope: &Scope<'_>) -> ExecCall {
    let arg = arguments(call).and_then(only_child);
    let lookup = |key| scope.value(field(arg?, key, script)?, script);
    let string = |key| lookup(key).and_then(|value| value.as_str().map(str::to_owned));
    match name {
        "exec_command" => {
            if let Some(command) = string("cmd").filter(|cmd| !cmd.trim().is_empty()) {
                if let Op::Shell(shell) = tool_ops::shell(&command, None, None, None) {
                    return ExecCall::Command { shell, cwd: string("workdir") };
                }
            }
        }
        "apply_patch" => {
            let text = arg.and_then(|arg| scope.value(arg, script))
                .and_then(|value| value.as_str().map(str::to_owned)).or_else(|| string("input"));
            if let Some(text) = text {
                let changes = super::tools::patch_ops(&text);
                if !changes.is_empty() {
                    return ExecCall::Patch { changes };
                }
            }
        }
        "view_image" => {
            if let Some(path) = string("path") {
                return ExecCall::Image { path };
            }
        }
        _ => {}
    }
    let keys: &[&str] = match name {
        "write_stdin" => &["session_id", "chars", "yield_time_ms", "max_output_tokens"],
        "web__run" => &["response_length"],
        "search_openai_docs" | "mcp__openaiDeveloperDocs__search_openai_docs" => &["query", "limit"],
        _ => &["title", "description", "path", "url", "query", "q", "prompt", "workdir", "timeout_ms"],
    };
    let mut fields: Vec<_> = keys.iter().filter_map(|key| lookup(key).map(|value| ((*key).to_owned(), value))).collect();
    let mut target = ["title", "description", "path", "url", "query", "q"].iter().find_map(|key| string(key));
    let mut action = None;
    let mut dynamic = arg.is_some_and(|arg| unparen(arg).kind() == "identifier");
    match name {
        "exec_command" | "apply_patch" | "view_image" => dynamic = true,
        "write_stdin" => {
            target = lookup("session_id").and_then(|id| id.as_u64().map(|id| id.to_string()));
            let chars = string("chars");
            action = chars.as_deref().map(|chars| if chars.is_empty() { "poll" } else { "send" });
            dynamic = target.is_none() || chars.is_none();
        }
        "web__run" => {
            for key in WEB_ACTIONS {
                let Some(entries) = arg.and_then(|arg| field(arg, key, script)) else { continue };
                action.get_or_insert(key);
                if entries.kind() != "array" {
                    dynamic = true;
                    continue;
                }
                let search = matches!(key, "search_query" | "image_query" | "literature_search_query");
                let mut values = Vec::new();
                for entry in children(entries).take(12) {
                    let lookup = |key| scope.value(field(entry, key, script)?, script);
                    let label = ["q", "ref_id", "pattern", "location", "ticker", "utc_offset"].into_iter()
                        .find_map(lookup).filter(Value::is_string);
                    if let Some(label) = label {
                        if let Some(label) = label.as_str() {
                            target.get_or_insert_with(|| label.into());
                        }
                        if search {
                            let domains = field(entry, "domains", script).map(|node| {
                                (node.kind() == "array").then(|| children(node)
                                    .map(|node| scope.value(node, script).filter(Value::is_string))
                                    .collect::<Option<Vec<_>>>()).flatten()
                            });
                            dynamic |= domains.as_ref().is_some_and(Option::is_none);
                            let recency = lookup("recency");
                            dynamic |= recency.is_none() && field(entry, "recency", script).is_some();
                            values.push(serde_json::json!({"query":label,"domains":domains.flatten(),"recency":recency}));
                        } else {
                            values.push(label);
                        }
                    } else if search {
                        dynamic = true;
                    }
                }
                if !values.is_empty() {
                    fields.push((key.into(), Value::Array(values)));
                }
            }
        }
        _ => {}
    }
    if let Some(target) = &mut target {
        *target = tool_ops::first_line(target);
    }
    let expression = arguments(call).filter(|_| dynamic).map(|args| source(args, script).to_owned());
    ExecCall::Tool { name: name.into(), action, target, fields, expression, dynamic }
}

fn is_function(kind: &str) -> bool {
    matches!(kind, "arrow_function" | "function_expression" | "function_declaration" | "method_definition" | "generator_function" | "generator_function_declaration")
}

fn has_nested_scope(node: Node<'_>) -> bool {
    let mut parent = node.parent();
    while let Some(node) = parent {
        if is_function(node.kind()) || node.kind() == "statement_block" {
            return true;
        }
        parent = node.parent();
    }
    false
}

fn push_feature(features: &mut Vec<ExecFeature>, feature: ExecFeature) {
    if !features.contains(&feature) {
        features.push(feature);
    }
}

struct Description<'s> {
    script: &'s str,
    calls: Vec<ExecCall>,
    features: Vec<ExecFeature>,
    scope: Scope<'s>,
    shadows_tools: bool,
}

impl<'s> Description<'s> {
    fn visit(&mut self, root: Node<'_>) {
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            if node.kind() == "call_expression" {
                self.call(node);
            }
            if node.kind() == "variable_declarator" && !has_nested_scope(node) {
                self.bind(node);
            }
            pending.extend(children(node).rev());
        }
    }

    fn bind(&mut self, node: Node<'_>) {
        let Some((name, value)) = constant_binding(node) else { return };
        let name = source(name, self.script);
        if let Some(value) = self.scope.value(value, self.script) {
            self.scope.constants.insert(name, value);
        }
        if !self.shadows_tools {
            if let Some(tool) = tool_name(value, self.script) {
                self.scope.aliases.insert(name, tool);
            }
        }
    }

    fn call(&mut self, node: Node<'_>) {
        let Some(function) = node.child_by_field_name("function") else { return };
        let scope = Scope::default();
        let scope = if has_nested_scope(node) { &scope } else { &self.scope };
        let raw = source(function, self.script);
        let name = if self.shadows_tools { None } else {
            tool_name(function, self.script).or_else(|| scope.aliases.get(raw).cloned())
        };
        if let Some(name) = name {
            self.calls.push(decode_call(&name, node, self.script, scope));
            self.call_features(node, function);
        }
        match raw {
            "load" | "store" => push_feature(&mut self.features, ExecFeature::State),
            "text" | "image" | "audio" | "generatedImage" | "notify" => push_feature(&mut self.features, ExecFeature::Output),
            _ => {}
        }
    }

    fn call_features(&mut self, node: Node<'_>, function: Node<'_>) {
        if children(node).chain(children(function)).any(|child| child.kind() == "optional_chain") {
            push_feature(&mut self.features, ExecFeature::Conditional);
        }
        let mut ancestor = node.parent();
        while let Some(node) = ancestor {
            let feature = match node.kind() {
                "for_statement" | "for_in_statement" | "while_statement" | "do_statement" => Some(ExecFeature::Loop),
                "if_statement" | "ternary_expression" | "switch_statement" | "catch_clause" => Some(ExecFeature::Conditional),
                "call_expression" => node.child_by_field_name("function").and_then(|function| {
                    if matches!(source(function, self.script), "Promise.all" | "Promise.allSettled" | "Promise.race" | "Promise.any") {
                        return Some(ExecFeature::Parallel);
                    }
                    function.child_by_field_name("property")
                        .filter(|property| matches!(source(*property, self.script), "map" | "flatMap" | "forEach"))
                        .map(|_| ExecFeature::Loop)
                }),
                _ => None,
            };
            if let Some(feature) = feature {
                push_feature(&mut self.features, feature);
            }
            ancestor = node.parent();
        }
    }
}

fn has_tool_binding(root: Node<'_>, script: &str) -> bool {
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if matches!(node.kind(), "identifier" | "shorthand_property_identifier_pattern") && source(node, script) == "tools" {
            let mut binding = node;
            while let Some(parent) = binding.parent() {
                match parent.kind() {
                    "formal_parameters" => return true,
                    "variable_declarator" | "function_declaration" | "function_expression" |
                    "arrow_function" | "catch_clause" | "class_declaration" => {
                        if ["name", "parameter"].into_iter().any(|field| parent.child_by_field_name(field) == Some(binding)) {
                            return true;
                        }
                        break;
                    }
                    "object_pattern" | "array_pattern" | "pair_pattern" | "assignment_pattern" | "rest_pattern" => binding = parent,
                    _ => break,
                }
            }
        }
        pending.extend(children(node));
    }
    false
}

pub(super) fn describe(script: &str) -> Op {
    let lines = line_count(script);
    let Some(tree) = parse(script) else {
        return Op::Exec { lines, calls: vec![], features: vec![] };
    };
    let root = tree.root_node();
    if root.has_error() {
        return Op::Exec { lines, calls: vec![], features: vec![ExecFeature::Invalid] };
    }
    let mut description = Description {
        script, calls: Vec::new(), features: Vec::new(), scope: Scope::default(),
        shadows_tools: script.contains("tools") && has_tool_binding(root, script),
    };
    description.visit(root);
    Op::Exec { lines, calls: description.calls, features: description.features }
}

fn literal_arguments(node: Node<'_>, script: &str) -> bool {
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        match node.kind() {
            "arguments" | "object" | "array" => pending.extend(children(node)),
            "pair" => {
                let Some(key) = node.child_by_field_name("key") else { return false };
                if key.kind() == "computed_property_name" || key_name(key, script).is_none() { return false; }
                pending.extend(node.child_by_field_name("value"));
            }
            "string" | "template_string" if literal_string(node, script).is_some() => {}
            "number" | "true" | "false" | "null" => {}
            _ => return false,
        }
    }
    true
}

pub(super) fn wrapped_execution(script: &str) -> Option<ExecutionKey> {
    let tree = parse(script)?;
    let root = tree.root_node();
    if root.has_error() { return None; }
    let statement = only_child(root)?;
    if statement.kind() != "expression_statement" { return None; }
    let mut expression = unparen(only_child(statement)?);
    if expression.kind() == "call_expression" && source(expression.child_by_field_name("function")?, script) == "text" {
        expression = unparen(only_child(arguments(expression)?)?);
    }
    if expression.kind() != "await_expression" { return None; }
    let call = unparen(only_child(expression)?);
    if call.kind() != "call_expression" || call.child_by_field_name("optional_chain").is_some() { return None; }
    let function = call.child_by_field_name("function")?;
    if children(function).any(|node| node.kind() == "optional_chain") { return None; }
    let name = tool_name(function, script)?;
    let arguments = arguments(call)?;
    if !literal_arguments(arguments, script) { return None; }
    let arg = only_child(arguments)?;
    let string = |key| literal_string(field(arg, key, script)?, script);
    Some(match name.as_ref() {
        "exec_command" => ExecutionKey::Command(string("cmd")?),
        "apply_patch" => {
            let patch = literal_string(arg, script).or_else(|| string("input"))?;
            let mut paths: Vec<_> = super::tools::patch_ops(&patch).into_iter().filter_map(|op| match op {
                Op::Write { path, .. } | Op::Edit { path, .. } | Op::Delete { path } => Some(path),
                _ => None,
            }).collect();
            if paths.is_empty() { return None; }
            paths.sort_unstable();
            paths.dedup();
            ExecutionKey::Patch(paths)
        }
        "view_image" => {
            let path = string("path")?;
            ExecutionKey::Image(path.strip_prefix("file://").unwrap_or(&path).into())
        }
        "web__run" => {
            let mut actions = WEB_ACTIONS.into_iter().filter_map(|key| field(arg, key, script).map(|node| (key, node)));
            let (action, entries) = actions.next()?;
            if actions.next().is_some() || entries.kind() != "array" || only_child(entries).is_none() { return None; }
            ExecutionKey::Web(match action {
                "search_query" => "search",
                "open" => "open",
                "find" => "find",
                _ => return None,
            })
        }
        "clock__sleep" => ExecutionKey::Sleep,
        other => {
            let (server, tool) = other.strip_prefix("mcp__")?.split_once("__")?;
            ExecutionKey::Mcp(server.into(), tool.into())
        }
    })
}
