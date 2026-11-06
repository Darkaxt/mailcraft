// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lifecycle tools use the authenticated native backend, never desktop input.

use serde_json::{Value, json};

use crate::Backend;
use crate::tools::{Policy, call_tool, tool_definitions};

#[derive(Default)]
struct Desktop(Vec<(String, Value)>);

impl Backend for Desktop {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.0.push((method.to_string(), params));
        Ok(json!({"requested": true}))
    }
    fn has_ui(&self) -> bool {
        true
    }
    fn describe(&self) -> String {
        "synthetic app".into()
    }
}

#[test]
fn lifecycle_tools_are_discovered_and_call_only_the_native_action() {
    let definitions = tool_definitions();
    for (name, action) in [("hide_app", "hide"), ("close_app", "close")] {
        assert!(definitions.as_array().unwrap().iter().any(|tool| tool["name"] == name));
        let mut backend = Desktop::default();
        let result = call_tool(&mut backend, &Policy::default(), name, &json!({}));
        assert!(!result.is_error, "{:?}", result.content);
        assert_eq!(backend.0, vec![("app.lifecycle".into(), json!({"action": action}))]);
        let mut headless = crate::Headless::default();
        let result = call_tool(&mut headless, &Policy::default(), name, &json!({}));
        assert!(result.is_error);
    }
}
