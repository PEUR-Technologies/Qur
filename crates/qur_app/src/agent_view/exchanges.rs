//! What each turn was about, read from the chat for commit messages. Kept
//! apart so the chat's storage can change behind this one accessor.

use super::*;

impl AgentTask {
    /// For each turn line still in the chat (turns that changed files): its
    /// number, your message that started it and the agent's last message.
    pub fn turn_exchanges(&self) -> Vec<(u32, Option<String>, Option<String>)> {
        let mut out = Vec::new();
        let (mut prompt, mut reply): (Option<&str>, Option<&str>) = (None, None);
        for item in &self.chat {
            match item {
                ChatItem::User(t) => (prompt, reply) = (Some(t.as_str()), None),
                ChatItem::Agent(t) if !t.trim().is_empty() => reply = Some(t.as_str()),
                ChatItem::Turn(n) => {
                    out.push((*n, prompt.map(str::to_string), reply.map(str::to_string)));
                    reply = None;
                }
                _ => {}
            }
        }
        out
    }
}
