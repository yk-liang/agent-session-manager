//! Codex `exec` 的 JavaScript 单元：静态找出其中的 `tools.*` 调用，参数求得出时按同名工具单独调用的规则描述。
mod ast;

use super::super::tool_ops::{first_line, line_count, text, ExecCall, ExecFeature, Op};
use ast::{
    arguments, body_written_names, children, creates_scope, hoisted_names, is_field, key_name,
    literal_string, member_call, mutating_call_target, only_child, parse, pattern_names,
    root_identifier, scope_names, source, template, unescape, unparen, MAX_SOURCE_BYTES,
};
use serde_json::{Map, Value};
use std::{borrow::Cow, collections::{HashMap, HashSet}};
use tree_sitter::Node;

/// 遍历字面量数组的循环按元素展开成多次调用，元素再多或累计展开超限时只保留一次带表达式的调用。
const MAX_EXPANSION: usize = 12;
const MAX_EXPANDED_STEPS: usize = 64;
const MAX_CALLS: usize = 256;
const MAX_VISITED: usize = 20_000;
const MAX_EVAL_BYTES: usize = 4 * MAX_SOURCE_BYTES;

fn push_feature(features: &mut Vec<ExecFeature>, feature: ExecFeature) {
    if !features.contains(&feature) {
        features.push(feature);
    }
}

fn value_bytes(value: &Value) -> usize {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => 16,
        Value::String(text) => 16 + text.len(),
        Value::Array(items) => 16 + items.iter().map(value_bytes).sum::<usize>(),
        Value::Object(map) => 16 + map.iter().map(|(key, value)| key.len() + value_bytes(value)).sum::<usize>(),
    }
}

struct BudgetWriter {
    remaining: usize,
}

impl std::io::Write for BudgetWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > self.remaining {
            return Err(std::io::Error::other("budget exceeded"));
        }
        self.remaining -= buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

enum Binding<'s> {
    Const { value: Value, exact: bool, cost: usize, obj: Option<usize> },
    ToolAlias(Cow<'s, str>),
    ToolsRoot,
    Unknown,
}

enum Task<'t, 's> {
    Visit(Node<'t>),
    EnterScope(Vec<(&'s str, Binding<'s>)>),
    LeaveScope,
    EndExpansion,
}

struct Description<'s> {
    script: &'s str,
    calls: Vec<ExecCall>,
    features: Vec<ExecFeature>,
    scopes: Vec<HashMap<&'s str, Binding<'s>>>,
    /// 容器对象间的无向引用边（如 `arr = [a]`），修改外层或内层对象时互相关联失效。
    next_obj: usize,
    object_refs: HashMap<usize, Vec<usize>>,
    poisoned: HashSet<usize>,
    tools_poisoned: bool,
    eval_budget: usize,
    expansion_budget: usize,
    expansion_depth: usize,
    visit_budget: usize,
}

impl<'s> Description<'s> {
    fn new(script: &'s str) -> Self {
        Description {
            script,
            calls: Vec::new(),
            features: Vec::new(),
            scopes: Vec::new(),
            next_obj: 1,
            object_refs: HashMap::new(),
            poisoned: HashSet::new(),
            tools_poisoned: false,
            eval_budget: MAX_EVAL_BYTES,
            expansion_budget: MAX_EXPANDED_STEPS,
            expansion_depth: 0,
            visit_budget: MAX_VISITED,
        }
    }

    fn charge(&mut self, bytes: usize) -> bool {
        if bytes > self.eval_budget {
            return false;
        }
        self.eval_budget -= bytes;
        true
    }

    fn binding(&self, name: &str) -> Option<&Binding<'s>> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    fn invalidate_binding(&mut self, name: &str) {
        if let Some(slot) = self.scopes.iter_mut().rev().find_map(|scope| scope.get_mut(name)) {
            *slot = Binding::Unknown;
        } else if name == "tools" {
            self.tools_poisoned = true;
        }
    }

    fn link_object(&mut self, id: usize, refs: &[usize]) {
        for &other in refs {
            self.object_refs.entry(id).or_default().push(other);
            self.object_refs.entry(other).or_default().push(id);
        }
    }

    fn poison_object(&mut self, root_id: usize) {
        let mut stack = vec![root_id];
        while let Some(id) = stack.pop() {
            if self.poisoned.insert(id) {
                if let Some(children) = self.object_refs.get(&id) {
                    stack.extend(children.iter().copied());
                }
            }
        }
    }

    fn poison_target(&mut self, name: &str) {
        match self.binding(name) {
            None if name == "tools" => self.tools_poisoned = true,
            Some(Binding::ToolsRoot) => self.tools_poisoned = true,
            Some(Binding::Const { obj: Some(id), .. }) => self.poison_object(*id),
            _ => {}
        }
    }

    fn is_tools_root(&self, name: &str) -> bool {
        if self.tools_poisoned {
            return false;
        }
        match self.binding(name) {
            None => name == "tools",
            Some(Binding::ToolsRoot) => true,
            _ => false,
        }
    }

    fn lookup(&mut self, name: &str, exact: &mut bool, refs: &mut Vec<usize>) -> Option<Value> {
        let (value, complete, cost, obj) = match self.binding(name)? {
            Binding::Const { value, exact, cost, obj } => (value, *exact, *cost, *obj),
            Binding::ToolAlias(_) | Binding::ToolsRoot | Binding::Unknown => return None,
        };
        if obj.is_some_and(|id| self.poisoned.contains(&id)) || cost > self.eval_budget {
            return None;
        }
        let cloned = value.clone();
        self.eval_budget -= cost;
        *exact &= complete;
        if let Some(id) = obj {
            refs.push(id);
        }
        Some(cloned)
    }

    fn resolve_tool(&self, node: Node<'_>) -> Option<Cow<'s, str>> {
        let node = unparen(node);
        if let Some((object, name)) = member_call(node, self.script) {
            if self.is_tools_root(object) {
                return Some(name);
            }
        }
        if node.kind() == "identifier" {
            if let Some(Binding::ToolAlias(name)) = self.binding(source(node, self.script)) {
                return Some(name.clone());
            }
        }
        None
    }

    /// 参数的静态值。字符串里运行时才知道的部分写成 `${源码}`，此时 `exact` 置为 false。
    fn eval(&mut self, node: Node<'_>, exact: &mut bool, depth: usize, refs: &mut Vec<usize>) -> Option<Value> {
        if depth > 16 || self.eval_budget == 0 {
            return None;
        }
        let node = unparen(node);
        let script = self.script;
        match node.kind() {
            "identifier" | "shorthand_property_identifier" => self.lookup(source(node, script), exact, refs),
            "number" => {
                if !self.charge(16) { return None; }
                serde_json::from_str(&source(node, script).replace('_', "")).ok()
            }
            "true" => self.charge(16).then_some(Value::Bool(true)),
            "false" => self.charge(16).then_some(Value::Bool(false)),
            "null" => self.charge(16).then_some(Value::Null),
            "string" => {
                if node.byte_range().len() > self.eval_budget.saturating_add(16) {
                    return None;
                }
                let text = literal_string(node, script)?;
                self.charge(16 + text.len()).then_some(Value::String(text))
            }
            "binary_expression" if node.child_by_field_name("operator")?.kind() == "+" => {
                let left_node = node.child_by_field_name("left")?;
                let right_node = node.child_by_field_name("right")?;
                let (left, left_known, left_text) = self.eval_concat_part(left_node, depth + 1, refs)?;
                let (right, right_known, right_text) = self.eval_concat_part(right_node, depth + 1, refs)?;
                if (!left_text && !right_text) || !self.charge(16 + left.len() + right.len()) {
                    return None;
                }
                *exact &= left_known && right_known;
                Some(Value::String(left + &right))
            }
            "object" => {
                if !self.charge(16) { return None; }
                let mut object = Map::new();
                for property in children(node) {
                    match property.kind() {
                        "spread_element" => {
                            let mut spread_exact = true;
                            match only_child(property).and_then(|child| self.eval(child, &mut spread_exact, depth + 1, refs)) {
                                Some(Value::Object(map)) => {
                                    if !spread_exact {
                                        *exact = false;
                                        object.clear();
                                    }
                                    object.extend(map);
                                }
                                _ => {
                                    *exact = false;
                                    object.clear();
                                }
                            }
                        }
                        "pair" => match property.child_by_field_name("key").and_then(|key| key_name(key, script)) {
                            Some(key) => match property.child_by_field_name("value").and_then(|value| self.eval(value, exact, depth + 1, refs)) {
                                Some(value) if self.charge(key.len()) => { object.insert(key.into_owned(), value); }
                                _ => {
                                    *exact = false;
                                    object.remove(key.as_ref());
                                }
                            },
                            None => {
                                *exact = false;
                                object.clear();
                            }
                        },
                        "shorthand_property_identifier" => {
                            let key = source(property, script);
                            match self.eval(property, exact, depth + 1, refs) {
                                Some(value) if self.charge(key.len()) => { object.insert(key.into(), value); }
                                _ => {
                                    *exact = false;
                                    object.remove(key);
                                }
                            }
                        }
                        _ => {
                            *exact = false;
                            match property.child_by_field_name("name").and_then(|key| key_name(key, script)) {
                                Some(key) => { object.remove(key.as_ref()); }
                                None => object.clear(),
                            }
                        }
                    }
                }
                Some(Value::Object(object))
            }
            "array" => {
                if !self.charge(16) { return None; }
                let mut elements = Vec::new();
                let mut expect_value = true;
                for index in 0..node.child_count() {
                    let child = node.child(index as u32)?;
                    match child.kind() {
                        "[" | "]" | "comment" => {}
                        "," => {
                            if expect_value {
                                if !self.charge(16) { return None; }
                                elements.push(Value::Null);
                            }
                            expect_value = true;
                        }
                        "spread_element" => {
                            let mut spread_exact = true;
                            match only_child(child).and_then(|inner| self.eval(inner, &mut spread_exact, depth + 1, refs)) {
                                Some(Value::Array(items)) if spread_exact => elements.extend(items),
                                _ => {
                                    *exact = false;
                                    return None;
                                }
                            }
                            expect_value = false;
                        }
                        _ => {
                            match self.eval(child, exact, depth + 1, refs) {
                                Some(value) => elements.push(value),
                                None => {
                                    *exact = false;
                                    if !self.charge(16) { return None; }
                                    elements.push(Value::Null);
                                }
                            }
                            expect_value = false;
                        }
                    }
                }
                Some(Value::Array(elements))
            }
            _ => {
                let (template, raw) = template(node, script)?;
                let mut text = String::new();
                let mut start = template.start_byte() + 1;
                for substitution in children(template).filter(|child| child.kind() == "template_substitution") {
                    if substitution.start_byte().saturating_sub(start) > self.eval_budget {
                        return None;
                    }
                    let chunk = unescape(&script[start..substitution.start_byte()], raw)?;
                    if !self.charge(chunk.len()) { return None; }
                    text += &chunk;
                    let expression = only_child(substitution)?;
                    let part = match self.eval(expression, exact, depth + 1, refs) {
                        Some(Value::String(part)) => Cow::Owned(part),
                        Some(Value::Number(number)) => Cow::Owned(number.to_string()),
                        _ => {
                            *exact = false;
                            Cow::Borrowed(source(substitution, script))
                        }
                    };
                    if !self.charge(part.len()) { return None; }
                    text += &part;
                    start = substitution.end_byte();
                }
                if template.end_byte().saturating_sub(1 + start) > self.eval_budget {
                    return None;
                }
                let tail = unescape(&script[start..template.end_byte() - 1], raw)?;
                if !self.charge(16 + tail.len()) { return None; }
                text += &tail;
                Some(Value::String(text))
            }
        }
    }

    fn eval_concat_part(&mut self, node: Node<'_>, depth: usize, refs: &mut Vec<usize>) -> Option<(String, bool, bool)> {
        let mut known = true;
        Some(match self.eval(node, &mut known, depth, refs) {
            Some(Value::String(text)) => (text, known, true),
            Some(Value::Number(number)) => (number.to_string(), known, false),
            _ => {
                let raw = source(node, self.script);
                if raw.len() + 3 > self.eval_budget {
                    return None;
                }
                (format!("${{{raw}}}"), false, false)
            }
        })
    }

    fn visit(&mut self, root: Node<'_>) {
        let mut pending = vec![Task::Visit(root)];
        while let Some(task) = pending.pop() {
            let node = match task {
                Task::Visit(node) => {
                    if self.visit_budget == 0 {
                        break;
                    }
                    self.visit_budget -= 1;
                    node
                }
                Task::EnterScope(bindings) => {
                    self.scopes.push(bindings.into_iter().collect());
                    continue;
                }
                Task::LeaveScope => {
                    self.scopes.pop();
                    continue;
                }
                Task::EndExpansion => {
                    self.expansion_depth -= 1;
                    continue;
                }
            };
            match node.kind() {
                "call_expression" => self.call(node),
                "variable_declarator" => self.bind(node),
                "assignment_expression" | "augmented_assignment_expression" | "update_expression" | "unary_expression" => {
                    self.apply_mutation(node);
                }
                _ => {}
            }
            if let Some((items, body, iterations)) = self.expansion(node) {
                self.expansion_depth += 1;
                pending.push(Task::EndExpansion);
                for names in iterations.into_iter().rev() {
                    pending.push(Task::LeaveScope);
                    pending.push(Task::Visit(body));
                    pending.push(Task::EnterScope(names));
                }
                pending.push(Task::Visit(items));
                continue;
            }
            if creates_scope(node.kind()) {
                let bindings = scope_names(node, self.script).into_iter().map(|name| (name, Binding::Unknown)).collect();
                pending.push(Task::LeaveScope);
                pending.extend(children(node).rev().map(Task::Visit));
                pending.push(Task::EnterScope(bindings));
            } else {
                pending.extend(children(node).rev().map(Task::Visit));
            }
        }
    }

    fn apply_mutation(&mut self, node: Node<'_>) {
        match node.kind() {
            "assignment_expression" | "augmented_assignment_expression" => {
                if let Some(left) = node.child_by_field_name("left").map(unparen) {
                    if matches!(left.kind(), "member_expression" | "subscript_expression") {
                        if let Some(root) = root_identifier(left, self.script) {
                            self.poison_target(root);
                        }
                    } else {
                        let mut names = Vec::new();
                        pattern_names(left, self.script, &mut names);
                        for name in names {
                            self.invalidate_binding(name);
                        }
                    }
                }
            }
            "update_expression" => {
                if let Some(arg) = node.child_by_field_name("argument").map(unparen) {
                    if arg.kind() == "identifier" {
                        self.invalidate_binding(source(arg, self.script));
                    } else if let Some(root) = root_identifier(arg, self.script) {
                        self.poison_target(root);
                    }
                }
            }
            "unary_expression" if node.child_by_field_name("operator").is_some_and(|op| source(op, self.script) == "delete") => {
                if let Some(root) = node.child_by_field_name("argument").and_then(|arg| root_identifier(arg, self.script)) {
                    self.poison_target(root);
                }
            }
            _ => {}
        }
    }

    /// `for (const x of [...])` 和 `[...].map(x => …)`：数组能完全求出且循环变量未被改写时逐个展开。
    #[allow(clippy::type_complexity)]
    fn expansion<'t>(&mut self, node: Node<'t>) -> Option<(Node<'t>, Node<'t>, Vec<Vec<(&'s str, Binding<'s>)>>)> {
        let script = self.script;
        let (pattern, items, array, body, extra) = match node.kind() {
            "for_in_statement" if node.child_by_field_name("operator").is_some_and(|op| source(op, script) == "of") => {
                let right = node.child_by_field_name("right")?;
                (node.child_by_field_name("left")?, right, right, node.child_by_field_name("body")?, Vec::new())
            }
            "call_expression" => {
                let function = node.child_by_field_name("function")?;
                let method = function.child_by_field_name("property").map(|property| source(property, script));
                if function.kind() != "member_expression" || !matches!(method, Some("map" | "flatMap" | "forEach")) {
                    return None;
                }
                let callback = unparen(only_child(arguments(node)?)?);
                if !matches!(callback.kind(), "arrow_function" | "function_expression") {
                    return None;
                }
                let mut extra = Vec::new();
                if let Some(name) = callback.child_by_field_name("name").filter(|name| name.kind() == "identifier") {
                    extra.push(source(name, script));
                }
                let pattern = match callback.child_by_field_name("parameter") {
                    Some(parameter) => parameter,
                    None => {
                        let mut params = children(callback.child_by_field_name("parameters")?);
                        let first = params.next()?;
                        for rest in params {
                            pattern_names(rest, script, &mut extra);
                        }
                        first
                    }
                };
                let body = callback.child_by_field_name("body")?;
                hoisted_names(body, script, &mut extra);
                (pattern, function, function.child_by_field_name("object")?, body, extra)
            }
            _ => return None,
        };
        if self.expansion_depth > 0 || self.calls.len() >= MAX_CALLS {
            return None;
        }
        let mut bound = Vec::new();
        pattern_names(pattern, script, &mut bound);
        let written = body_written_names(body, script);
        if bound.iter().any(|name| written.contains(name)) {
            return None;
        }
        let mut exact = true;
        let mut refs = Vec::new();
        let Value::Array(values) = self.eval(array, &mut exact, 0, &mut refs)? else { return None };
        if !exact || values.len() > MAX_EXPANSION || values.len() > self.expansion_budget {
            return None;
        }
        let iterations = values.into_iter().map(|value| {
            let mut bindings: Vec<_> = extra.iter().map(|name| (*name, Binding::Unknown)).collect();
            bindings.extend(self.destructure(pattern, value, &refs)?);
            Some(bindings)
        }).collect::<Option<Vec<_>>>()?;
        self.expansion_budget -= iterations.len();
        Some((items, body, iterations))
    }

    fn destructure(&mut self, pattern: Node<'_>, value: Value, refs: &[usize]) -> Option<Vec<(&'s str, Binding<'s>)>> {
        let script = self.script;
        let mut make_binding = |value: Value| {
            let cost = value_bytes(&value);
            let obj = matches!(value, Value::Object(_) | Value::Array(_)).then(|| {
                let id = self.next_obj;
                self.next_obj += 1;
                self.link_object(id, refs);
                id
            });
            Binding::Const { value, exact: true, cost, obj }
        };
        match pattern.kind() {
            "identifier" => Some(vec![(source(pattern, script), make_binding(value))]),
            "array_pattern" => {
                let Value::Array(values) = value else { return None };
                let mut slot = 0;
                let mut names = Vec::new();
                for index in 0..pattern.child_count() {
                    let child = pattern.child(index as u32)?;
                    match child.kind() {
                        "," => slot += 1,
                        "identifier" => names.push((source(child, script), make_binding(values.get(slot).cloned().unwrap_or(Value::Null)))),
                        "[" | "]" | "comment" => {}
                        _ => return None,
                    }
                }
                Some(names)
            }
            "object_pattern" => {
                if !value.is_object() {
                    return None;
                }
                children(pattern).map(|name| (name.kind() == "shorthand_property_identifier_pattern")
                    .then(|| (source(name, script), make_binding(value[source(name, script)].clone())))).collect()
            }
            _ => None,
        }
    }

    fn bind(&mut self, node: Node<'_>) {
        let is_const = node.parent().and_then(|parent| parent.child_by_field_name("kind")).is_some_and(|kind| kind.kind() == "const");
        let Some(name_node) = node.child_by_field_name("name") else { return };
        let Some(value_node) = node.child_by_field_name("value").map(unparen) else { return };
        if name_node.kind() != "identifier" {
            let mut exact = true;
            let mut refs = Vec::new();
            let evaluated = self.eval(value_node, &mut exact, 0, &mut refs);
            if is_const {
                if let Some(bindings) = evaluated.and_then(|value| self.destructure(name_node, value, &refs)) {
                    if let Some(scope) = self.scopes.last_mut() {
                        scope.extend(bindings);
                    }
                    return;
                }
            }
            if !refs.is_empty() {
                let mut names = Vec::new();
                pattern_names(name_node, self.script, &mut names);
                for name in names {
                    let id = self.next_obj;
                    self.next_obj += 1;
                    self.link_object(id, &refs);
                    if let Some(scope) = self.scopes.last_mut() {
                        scope.insert(name, Binding::Const { value: Value::Null, exact: false, cost: usize::MAX, obj: Some(id) });
                    }
                }
            }
            return;
        }
        let name = source(name_node, self.script);
        if !is_const {
            let mut exact = true;
            let mut refs = Vec::new();
            let _ = self.eval(value_node, &mut exact, 0, &mut refs);
            if !refs.is_empty() {
                let id = self.next_obj;
                self.next_obj += 1;
                self.link_object(id, &refs);
                if let Some(scope) = self.scopes.last_mut() {
                    scope.insert(name, Binding::Const { value: Value::Null, exact: false, cost: usize::MAX, obj: Some(id) });
                }
            }
            return;
        }
        let binding = if value_node.kind() == "identifier" && self.is_tools_root(source(value_node, self.script)) {
            Binding::ToolsRoot
        } else if let Some(tool) = self.resolve_tool(value_node) {
            if self.charge(tool.len()) { Binding::ToolAlias(tool) } else { Binding::Unknown }
        } else {
            let mut exact = true;
            let mut refs = Vec::new();
            match self.eval(value_node, &mut exact, 0, &mut refs) {
                Some(value) => {
                    let cost = value_bytes(&value);
                    let obj = if matches!(value, Value::Array(_) | Value::Object(_)) {
                        if value_node.kind() == "identifier" && refs.len() == 1 {
                            Some(refs[0])
                        } else {
                            let id = self.next_obj;
                            self.next_obj += 1;
                            self.link_object(id, &refs);
                            Some(id)
                        }
                    } else {
                        None
                    };
                    Binding::Const { value, exact, cost, obj }
                }
                None => Binding::Unknown,
            }
        };
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, binding);
        }
    }

    fn call(&mut self, node: Node<'_>) {
        if let Some(target) = mutating_call_target(node, self.script) {
            self.poison_target(target);
        }
        let Some(function) = node.child_by_field_name("function") else { return };
        if let Some(name) = self.resolve_tool(function) {
            if self.calls.len() < MAX_CALLS && self.charge(name.len()) {
                let call = self.decode(&name, node);
                self.calls.push(call);
            }
            self.call_features(node, function);
        }
        if matches!(source(function, self.script), "load" | "store") {
            push_feature(&mut self.features, ExecFeature::State);
        }
    }

    fn decode(&mut self, name: &str, call: Node<'_>) -> ExecCall {
        let args = arguments(call);
        let mut exact = args.is_some();
        let mut refs = Vec::new();
        let input = match args {
            Some(args) if children(args).next().is_none() => Value::Null,
            Some(args) => only_child(args).and_then(|arg| self.eval(arg, &mut exact, 0, &mut refs)).unwrap_or_else(|| {
                exact = false;
                Value::Null
            }),
            None => Value::Null,
        };
        let expression = args.filter(|_| !exact).and_then(|args| {
            let raw = source(args, self.script);
            self.charge(raw.len()).then(|| raw.into())
        });
        // 不在 exec 内部递归解析 `tools.exec`，并在构建前按输入规模检查剩余预算；未求出的调用若连源码表达式也超出预算则不保留部分操作。
        let mut ops = if name == "exec" || (!exact && expression.is_none()) || value_bytes(&input) > self.eval_budget {
            Vec::new()
        } else {
            super::tools::describe(name, &input)
        };
        if !ops.is_empty() {
            let mut writer = BudgetWriter { remaining: self.eval_budget };
            if serde_json::to_writer(&mut writer, &ops).is_ok() {
                self.eval_budget = writer.remaining;
            } else {
                ops.clear();
            }
        }
        let target = ops.is_empty().then(|| ["title", "description", "path", "url", "query", "q", "prompt", "justification"]
            .into_iter().find_map(|key| text(&input, key)).map(first_line)).flatten();
        ExecCall {
            tool: name.into(),
            cwd: (name == "exec_command").then(|| text(&input, "workdir").map(Into::into)).flatten(),
            target,
            ops,
            input,
            expression,
        }
    }

    /// 调用在循环体、并发包装或条件分支里才算；`for (… of await Promise.all([…]))` 的迭代对象只求值一次。
    fn call_features(&mut self, node: Node<'_>, function: Node<'_>) {
        if children(node).chain(children(function)).any(|child| child.kind() == "optional_chain") {
            push_feature(&mut self.features, ExecFeature::Conditional);
        }
        let mut child = node;
        while let Some(parent) = child.parent() {
            let feature = match parent.kind() {
                "for_in_statement" if is_field(parent, "body", child) => Some(ExecFeature::Loop),
                "for_statement" if !is_field(parent, "initializer", child) => Some(ExecFeature::Loop),
                "while_statement" | "do_statement" => Some(ExecFeature::Loop),
                "if_statement" | "ternary_expression" if !is_field(parent, "condition", child) => Some(ExecFeature::Conditional),
                "switch_statement" if !is_field(parent, "value", child) => Some(ExecFeature::Conditional),
                "catch_clause" => Some(ExecFeature::Conditional),
                "binary_expression" if is_field(parent, "right", child)
                    && parent.child_by_field_name("operator").is_some_and(|op| matches!(op.kind(), "&&" | "||" | "??")) => Some(ExecFeature::Conditional),
                "call_expression" if is_field(parent, "arguments", child) => parent.child_by_field_name("function").and_then(|function| {
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
            child = parent;
        }
    }
}

/// 求不出操作的调用：补丁和命令仍按改动、运行算，其余不抢主动作。
fn fallback_weight(tool: &str) -> u8 {
    match tool {
        "apply_patch" => 6,
        "exec_command" => 5,
        _ => 0,
    }
}

/// 主动作：权重最高的操作，同级取先出现的。
fn lead(calls: &[ExecCall]) -> Option<(usize, usize)> {
    calls.iter().enumerate().flat_map(|(index, call)| {
        let fallback = call.ops.is_empty().then(|| (index, 0, fallback_weight(&call.tool)));
        call.ops.iter().enumerate().map(move |(op, value)| (index, op, value.weight())).chain(fallback)
    }).rev().max_by_key(|(_, _, weight)| *weight).map(|(call, op, _)| (call, op))
}

pub(super) fn describe(script: &str) -> Op {
    let lines = line_count(script);
    let Some(tree) = parse(script) else {
        return Op::Exec { lines, calls: vec![], lead: None, features: vec![] };
    };
    let root = tree.root_node();
    if root.has_error() {
        return Op::Exec { lines, calls: vec![], lead: None, features: vec![ExecFeature::Invalid] };
    }
    let mut description = Description::new(script);
    description.visit(root);
    let Description { calls, features, .. } = description;
    Op::Exec { lines, lead: lead(&calls), calls, features }
}

/// 只调用一次工具、参数全能求出的包装脚本：`await tools.x(…)` 或 `text(await tools.x(…))`。
pub(super) fn wrapper(script: &str) -> Option<ExecCall> {
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
    let function = call.child_by_field_name("function")?;
    if call.kind() != "call_expression" || children(call).chain(children(function)).any(|node| node.kind() == "optional_chain") {
        return None;
    }
    let (object, name) = member_call(function, script)?;
    if object != "tools" {
        return None;
    }
    let call = Description::new(script).decode(&name, call);
    call.expression.is_none().then_some(call)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exec_calls_reuse_native_tool_descriptions() {
        let script = r#"const dir = "src";
const r = await Promise.allSettled([
  tools.write_stdin({ session_id: 42, chars: "" }),
  tools.exec_command({ cmd: `rg -n needle ${dir} && cargo test`, workdir: "/repo", justification: "Run tests?" }),
  tools.exec_command({ cmd: `ls ${path}` }),
]);
for (const x of r) text(x);
for (const cmd of ["git status", "git diff"]) await tools.exec_command({ cmd });
await tools.write_stdin({ session_id: 42, chars: "\u0003" });"#;
        let op = describe(script);
        let json = json!(op);
        let calls = json["calls"].as_array().unwrap();
        assert_eq!(calls[0]["ops"], json!(super::super::tools::describe("write_stdin", &json!({"session_id":42,"chars":""}))));
        assert_eq!(calls[0]["ops"][0]["action"], "poll");
        assert_eq!(calls[1]["ops"][0]["intent"], "test");
        assert_eq!(calls[1]["ops"][0]["description"], "Run tests?");
        assert_eq!(calls[1]["cwd"], "/repo");
        assert!(calls[1].get("expression").is_none());
        assert_eq!(calls[2]["ops"][0]["command"], "ls ${path}");
        assert_eq!(calls[2]["expression"], "({ cmd: `ls ${path}` })");
        assert_eq!([&calls[3]["ops"][0]["command"], &calls[4]["ops"][0]["command"]], ["git status", "git diff"]);
        assert_eq!(calls[5]["ops"][0]["action"], "interrupt");
        assert_eq!(json["lead"], json!([1, 0]));
        assert_eq!(json["features"], json!(["parallel", "loop"]));
        let summary = json!(op.summary());
        assert!(summary["calls"][1].get("input").is_none() && summary["calls"][2].get("expression").is_none());

        let scoped = r#"const base = { workdir: "/repo", cmd: "cat a" };
const kept = { cmd: "cat kept" };
{ const kept = { cmd: "cat inner" }; kept.cmd = "cat mutated"; }
const aliased = { cmd: "cat a" };
const arr = [aliased];
arr[0].cmd = "cat b";
const queries = [{ q: "first" }];
const req = { search_query: queries };
queries.push({ q: "second" });
const destructured = { search_query: [{ q: "old" }] };
const { search_query } = destructured;
search_query.push({ q: "new" });
const outer = "cat outer";
function helper(tools) { return tools.exec_command({ cmd: "ignored" }); }
function defaulted({ tools = other }) { return tools.exec_command({ cmd: "ignored" }); }
["cat z"].map((cmd, tools) => tools.exec_command({ cmd }));
for (const cmd of ["cat a"]) { { const cmd = "cat b"; await tools.exec_command({ cmd }); } }
for (const outer of load("xs")) await tools.exec_command({ cmd: outer });
for (const [, cmd] of [["cat a", "cat c"], [, "cat d"]]) await tools.exec_command({ cmd });
for (const cmd of []) await tools.exec_command({ cmd });
for (const cmd of ["pwd"].missing) await tools.exec_command({ cmd });
for (let cmd of ["pwd"]) { cmd = "git status"; await tools.exec_command({ cmd }); }
await tools.exec_command(kept);
await tools.exec_command(aliased);
await tools.web__run(req);
await tools.web__run(destructured);
await tools.exec_command({ ...base, cmd: "cat e" });
await tools.exec_command({ cmd: "cat a", ...load("overrides") });
await tools.exec_command({ cmd: "cat a", cmd: load("next") });
const t = tools;
t.exec_command = fake;
await tools.exec_command({ cmd: "rebound" });"#;
        let scoped = json!(describe(scoped));
        let calls = scoped["calls"].as_array().unwrap();
        assert_eq!(calls.len(), 13);
        assert_eq!(calls[0]["ops"][0]["steps"][0]["path"], "b");
        assert!(calls[1].get("ops").is_none());
        assert_eq!((calls[2]["ops"][0]["steps"][0]["path"].clone(), calls[3]["ops"][0]["steps"][0]["path"].clone()), (json!("c"), json!("d")));
        assert!(calls[4].get("ops").is_none() && calls[5].get("ops").is_none());
        assert_eq!(calls[6]["ops"][0]["steps"][0]["path"], "kept");
        assert!(calls[7].get("ops").is_none() && calls[8].get("ops").is_none() && calls[9].get("ops").is_none());
        assert_eq!((calls[10]["cwd"].clone(), calls[10]["ops"][0]["steps"][0]["path"].clone()), (json!("/repo"), json!("e")));
        assert!(calls[11].get("ops").is_none() && calls[12].get("ops").is_none());

        let nested = r#"const xs = [1,2,3,4,5,6,7,8,9,10,11,12];
const s0 = "0123456789abcdef";
const s1 = s0 + s0; const s2 = s1 + s1; const s3 = s2 + s2; const s4 = s3 + s3; const s5 = s4 + s4;
const s6 = s5 + s5; const s7 = s6 + s6; const s8 = s7 + s7; const s9 = s8 + s8; const s10 = s9 + s9;
const s11 = s10 + s10; const s12 = s11 + s11; const s13 = s12 + s12; const s14 = s13 + s13; const s15 = s14 + s14;
for (const a of xs) for (const b of xs) for (const c of xs) for (const d of xs) for (const e of xs) for (const f of xs)
  await tools.exec_command({ cmd: `echo ${a} ${s15}` });"#;
        let bounded = json!(describe(nested));
        assert_eq!(bounded["calls"].as_array().unwrap().len(), 12);
    }
}
