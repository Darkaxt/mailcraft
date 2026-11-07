// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Exact destination scoping and lossless local rule editor state.

use super::*;
use mailcraft_model::{Condition, RuleField};

/// Observe the complete native order for the incoming account, including rules
/// hidden by a destination filter. Moving still uses the existing rules.move.
pub(super) fn rule_order<'a>(mailbox: &'a Mailbox, rule: &Rule, account: Id) -> (usize, Option<&'a Rule>, Option<&'a Rule>) {
    let ordered: Vec<_> = mailbox.rules_in_order(account).into_iter().filter_map(|i| mailbox.rules.get(i)).collect();
    let Some(position) = ordered.iter().position(|r| r.id == rule.id) else { return (0, None, None) };
    let server = rule.account == account && rule.keep == Keep::Server;
    let same_list = |r: &&Rule| (r.account == account && r.keep == Keep::Server) == server;
    let previous = ordered.iter().take(position).rev().copied().find(same_list);
    let next = ordered.iter().skip(position + 1).copied().find(same_list);
    (position + 1, previous, next)
}

pub(super) fn rule_button(ui: &mut Ui, icon: &str, label: &str, enabled: bool) -> egui::Response {
    let response = ui.add_enabled(enabled, egui::Button::new("").min_size(vec2(22.0, 20.0)).frame(false));
    let tokens = crate::theme::Tokens::get(ui.ctx());
    crate::icons::paint(ui.painter(), response.rect.shrink(4.0), icon, if enabled { tokens.icon } else { tokens.text_disabled }, tokens.accent);
    if response.has_focus() {
        ui.painter().rect_stroke(response.rect, 3.0, egui::Stroke::new(1.5, tokens.focus_ring), egui::StrokeKind::Inside);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    response.on_hover_text(label)
}

impl Cond {
    pub(super) fn as_value(&self, kind: When) -> Value {
        if kind.is_list() {
            let values: Vec<_> = self.text.split('\n').map(|value| if kind == When::FromAny { value } else { value.trim() }).collect();
            return json!({"type": kind.code(), "value": values});
        }
        if self.advanced || self.regex || self.case_sensitive || matches!(kind, When::OriginalFrom | When::ReceivingAlias) {
            let field = match kind {
                When::From => "from",
                When::To => "to",
                When::Subject => "subject",
                When::OriginalFrom => "originalFrom",
                When::ReceivingAlias => "receivingAlias",
                _ => "body",
            };
            json!({"type": "text", "value": {"field": field, "pattern": self.text,
                "regex": self.regex, "caseSensitive": self.case_sensitive}})
        } else {
            json!({"type": kind.code(), "value": self.text})
        }
    }

    fn from_condition(condition: &Condition, exception: bool) -> Cond {
        let mut row = Cond { exception, ..Default::default() };
        match condition {
            Condition::FromContains(text) => {
                row.kind = When::From;
                row.text.clone_from(text);
            }
            Condition::FromAny(senders) => {
                row.kind = When::FromAny;
                row.text = senders.join("\n");
            }
            Condition::FromAddresses(addresses) | Condition::ToAddresses(addresses) => {
                row.kind = if matches!(condition, Condition::FromAddresses(_)) { When::FromAddresses } else { When::ToAddresses };
                row.text = addresses.join("\n");
            }
            Condition::ToContains(text) => {
                row.kind = When::To;
                row.text.clone_from(text);
            }
            Condition::SubjectContains(text) => {
                row.kind = When::Subject;
                row.text.clone_from(text);
            }
            Condition::BodyContains(text) => {
                row.kind = When::Body;
                row.text.clone_from(text);
            }
            Condition::HasAttachment => row.kind = When::Attachment,
            Condition::Importance(importance) => {
                row.kind = When::Importance;
                row.importance = *importance;
            }
            Condition::Text(text) => {
                row.advanced = true;
                row.kind = match text.field {
                    RuleField::From => When::From,
                    RuleField::To => When::To,
                    RuleField::Subject => When::Subject,
                    RuleField::Body => When::Body,
                    RuleField::OriginalFrom => When::OriginalFrom,
                    RuleField::ReceivingAlias => When::ReceivingAlias,
                };
                row.text.clone_from(&text.pattern);
                row.regex = text.regex;
                row.case_sensitive = text.case_sensitive;
            }
            Condition::Unless(_) => {}
        }
        row
    }
}

impl RulesForm {
    pub(super) fn listed_rules(&self, mailbox: &Mailbox) -> Vec<Rule> {
        match self.destination {
            Some(folder) => mailbox.rules.iter().filter(|rule| rule.targets(folder)).cloned().collect(),
            None => mailbox.rules_in_order(self.account).into_iter().filter_map(|i| mailbox.rules.get(i).cloned()).collect(),
        }
    }

    pub(super) fn edit_rule(&mut self, mailbox: &Mailbox, id: Id) {
        let Some(rule) = mailbox.rules.iter().find(|r| r.id == id) else { return };
        if rule.on_server() || rule.unanswered || !rule.known() || rule.read_only() {
            return;
        }
        if rule.conditions.iter().any(|condition| {
            matches!(condition, Condition::Unless(exceptions)
            if exceptions.iter().any(|condition| matches!(condition, Condition::Unless(_))))
        }) {
            self.error = tl!("Nested exceptions cannot be edited here. The original rule was preserved.").into();
            return;
        }
        self.choose(mailbox, rule.account);
        self.name.clone_from(&rule.name);
        self.conds = rule
            .conditions
            .iter()
            .flat_map(|condition| match condition {
                Condition::Unless(exceptions) => exceptions.iter().map(|c| Cond::from_condition(c, true)).collect::<Vec<_>>(),
                other => vec![Cond::from_condition(other, false)],
            })
            .collect();
        self.acts = rule
            .actions
            .iter()
            .map(|action| {
                let mut row = self.fresh_action();
                match action {
                    Action::MoveTo(folder) => {
                        row.kind = Do::Move;
                        row.folder = *folder;
                    }
                    Action::CopyTo(folder) => {
                        row.kind = Do::Copy;
                        row.folder = *folder;
                    }
                    Action::Categorize(category) => {
                        row.kind = Do::Categorize;
                        row.category.clone_from(category);
                    }
                    Action::Flag => row.kind = Do::Flag,
                    Action::MarkRead => row.kind = Do::MarkRead,
                    Action::SetImportance(importance) => {
                        row.kind = Do::SetImportance;
                        row.importance = *importance;
                    }
                    Action::Delete => row.kind = Do::Delete,
                }
                row
            })
            .collect();
        self.stop = rule.stop;
        self.server = false;
        self.editing = Some(id);
        self.run_in = None;
        self.then_run = false;
        self.preview = None;
        self.error.clear();
        self.open = true;
        self.focus = true;
    }
}
