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

    pub(super) fn from_condition(condition: &Condition, exception: bool) -> Cond {
        let mut row = Cond { exception, ..Default::default() };
        let words = |row: &mut Cond, kind: When, words: &[String]| {
            row.kind = kind;
            row.text = words.join("; ");
        };
        match condition {
            Condition::FromContains(t) => words(&mut row, When::From, t),
            Condition::ToContains(t) => words(&mut row, When::To, t),
            Condition::SubjectContains(t) => words(&mut row, When::Subject, t),
            Condition::BodyContains(t) => words(&mut row, When::Body, t),
            Condition::SubjectOrBodyContains(t) => words(&mut row, When::SubjectOrBody, t),
            Condition::From(p) => words(&mut row, When::Person, p),
            Condition::SentTo(p) => words(&mut row, When::SentTo, p),
            Condition::FromAny(senders) => {
                row.kind = When::FromAny;
                row.text = senders.join("\n");
            }
            Condition::FromAddresses(addresses) | Condition::ToAddresses(addresses) => {
                row.kind = if matches!(condition, Condition::FromAddresses(_)) { When::FromAddresses } else { When::ToAddresses };
                row.text = addresses.join("\n");
            }
            Condition::HasAttachment => row.kind = When::Attachment,
            Condition::Importance(importance) => {
                row.kind = When::Importance;
                row.importance = *importance;
            }
            Condition::Sensitivity(sensitivity) => {
                row.kind = When::Sensitivity;
                row.sensitivity = *sensitivity;
            }
            Condition::SizeBetween { min, max } => {
                row.kind = When::Size;
                row.min = min.to_string();
                row.max = max.to_string();
            }
            Condition::ReceivedBetween { from, to } => {
                row.kind = When::Received;
                row.from = from.as_ref().map(|d| d.iso()).unwrap_or_default();
                row.to = to.as_ref().map(|d| d.iso()).unwrap_or_default();
            }
            Condition::Category(names) => {
                row.kind = When::Category;
                row.category = names.first().cloned().unwrap_or_default();
            }
            Condition::SentOnlyToMe => row.kind = When::SentOnlyToMe,
            Condition::ToMe => row.kind = When::ToMe,
            Condition::CcMe => row.kind = When::CcMe,
            Condition::ToOrCcMe => row.kind = When::ToOrCcMe,
            Condition::NotToMe => row.kind = When::NotToMe,
            Condition::MeetingRequest => row.kind = When::Meeting,
            Condition::AutomaticReply => row.kind = When::AutomaticReply,
            Condition::Flagged => row.kind = When::Flagged,
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
        self.unless = rule.exceptions.iter().map(|c| Cond::from_condition(c, false)).collect();
        self.alert = rule.alert;
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
                    Action::Flag => {
                        row.kind = Do::Flag;
                        row.due = None;
                    }
                    Action::MarkRead => row.kind = Do::MarkRead,
                    Action::SetImportance(importance) => {
                        row.kind = Do::SetImportance;
                        row.importance = *importance;
                    }
                    Action::Delete => row.kind = Do::Delete,
                    Action::FlagFor(due) => {
                        row.kind = Do::Flag;
                        row.due = Some(*due);
                    }
                    Action::ClearFlag => row.kind = Do::ClearFlag,
                    Action::ClearCategories => row.kind = Do::ClearCategories,
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
