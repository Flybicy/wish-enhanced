//! Coalesced current-turn preview. No token/chunk log, and no disk persistence.
use crate::{protocol::StreamEvent, session::SessionEvent};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct LivePreview {
  pub revision: u64,
  events: Vec<Value>,
  positions: HashMap<String, usize>,
}
impl LivePreview {
  pub fn snapshot(&self, data: Value) -> Value {
    json!({"type":"snapshot", "data":data, "revision":self.revision, "live_events":self.events})
  }
  fn clear(&mut self) {
    self.events.clear();
    self.positions.clear();
  }
  /// Retains a pending approval in the turn preview: a client that subscribes
  /// or reconnects must still see the card that blocks the run, and a repeated
  /// note of the same call replaces the entry instead of stacking a duplicate.
  pub fn note_approval(
    &mut self,
    call_id: &str,
    name: &str,
    summary: &str,
    generation: u64,
  ) {
    let key = format!("approval:{call_id}");
    // Same externally tagged shape web_event serializations carry, so a
    // replaying client treats both alike.
    let entry = json!({"ApprovalRequired":{"call_id":call_id,"name":name,
      "generation":generation,"summary":summary}});
    match self.positions.get(&key) {
      Some(position) if self.events.get(*position).is_some() => {
        self.events[*position] = entry;
      }
      _ => {
        self.positions.insert(key, self.events.len());
        self.events.push(entry);
      }
    }
  }
  /// Drops the approval entry once the decision landed or the call was
  /// cancelled, so a reconnecting client never acts on a settled approval.
  pub fn retire_approval(&mut self, call_id: &str, generation: u64) {
    let key = format!("approval:{call_id}");
    let Some(position) = self.positions.remove(&key) else { return };
    let settled = match self.events.get(position) {
      Some(entry) => entry["ApprovalRequired"]["call_id"].as_str() == Some(call_id)
        && entry["ApprovalRequired"]["generation"].as_u64() == Some(generation),
      None => false,
    };
    if !settled {
      self.positions.insert(key, position);
      return;
    }
    self.events.remove(position);
    for slot in self.positions.values_mut() {
      if *slot > position {
        *slot -= 1;
      }
    }
  }
  pub fn observe(&mut self, event: &SessionEvent) {
    self.revision += 1;
    match event {
      SessionEvent::TurnStarted { .. } => {
        self.clear();
        self.events.push(json!(event));
      }
      SessionEvent::ResponseAccepted { .. }
      | SessionEvent::ResponseInterrupted(_)
      | SessionEvent::Finished(_) => self.clear(),
      // A live tool start is part of the turn preview: it is anchored per
      // call id so repeated starts of the same call coalesce instead of
      // stacking entries every turn.
      SessionEvent::ToolStarted(call) => {
        let key = format!("tool:{}", call.call_id);
        if !self.positions.contains_key(&key) {
          self.positions.insert(key, self.events.len());
          self.events.push(json!(event));
        }
      }
      SessionEvent::ModelStream(stream) => {
        let (kind, index, field, fragment) = match stream {
          StreamEvent::TextDelta { index, delta } => ("TextDelta", *index, "delta", delta),
          StreamEvent::ReasoningDelta { index, delta } => {
            ("ReasoningDelta", *index, "delta", delta)
          }
          StreamEvent::ReasoningDisplayDelta { index, delta } => {
            ("ReasoningDisplayDelta", *index, "delta", delta)
          }
          StreamEvent::ToolUseDelta { index, arguments, .. } => {
            ("ToolUseDelta", *index, "arguments", arguments)
          }
          StreamEvent::Usage(_) => {
            let value = json!(event);
            if let Some(position) = self.positions.get("usage") {
              self.events[*position] = value;
            } else {
              self.positions.insert("usage".into(), self.events.len());
              self.events.push(value);
            }
            return;
          }
          _ => return,
        };
        let key = format!("{kind}:{index}");
        if let Some(position) = self.positions.get(&key) {
          let value = &mut self.events[*position]["ModelStream"][kind];
          // Move the string out instead of copying the whole accumulated prefix per chunk.
          let mut text = match value[field].take() {
            Value::String(text) => text,
            _ => String::new(),
          };
          text.push_str(fragment);
          value[field] = json!(text);
          if let StreamEvent::ToolUseDelta { name, call_id, .. } = stream {
            if name.is_some() {
              value["name"] = json!(name);
            }
            if call_id.is_some() {
              value["call_id"] = json!(call_id);
            }
          }
        } else {
          self.positions.insert(key, self.events.len());
          self.events.push(json!(event));
        }
      }
      _ => {}
    }
  }
}

#[cfg(test)]
mod tests {
  use super::LivePreview;
  use serde_json::json;

  #[test]
  fn approval_preview_is_idempotent_and_generation_bound() {
    let mut preview = LivePreview::default();
    preview.note_approval("call-1", "shell_start", "echo [redacted]", 7);
    preview.note_approval("call-1", "shell_start", "echo [updated]", 7);
    let events = preview.snapshot(json!({}))["live_events"].as_array().unwrap().clone();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["ApprovalRequired"]["summary"], "echo [updated]");

    preview.retire_approval("call-1", 8);
    assert_eq!(preview.snapshot(json!({}))["live_events"].as_array().unwrap().len(), 1);
    preview.retire_approval("call-1", 7);
    assert!(preview.snapshot(json!({}))["live_events"].as_array().unwrap().is_empty());
  }
}
