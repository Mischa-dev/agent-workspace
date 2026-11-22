//! The words of an agent's reply as a harness streams them, checked against
//! the bounds of a message before each piece is kept, so rejected output is
//! never shown live or posted later.

/// The most a reply may hold, in characters: a message's own limit.
pub const MAX_REPLY_CHARACTERS: usize = 20_000;

/// Why a piece of a reply was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplyError {
    /// A control character other than a newline, return or tab.
    UnsupportedControlCharacter,
    /// The reply would pass [`MAX_REPLY_CHARACTERS`].
    TextTooLong,
}

/// A reply being streamed. A refused piece leaves it as it was.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReplyBuffer {
    text: String,
    characters: usize,
}

impl ReplyBuffer {
    pub fn push(&mut self, fragment: &str) -> Result<(), ReplyError> {
        if fragment
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
        {
            return Err(ReplyError::UnsupportedControlCharacter);
        }
        let characters = self
            .characters
            .checked_add(fragment.chars().count())
            .ok_or(ReplyError::TextTooLong)?;
        if characters > MAX_REPLY_CHARACTERS {
            return Err(ReplyError::TextTooLong);
        }
        self.text.push_str(fragment);
        self.characters = characters;
        Ok(())
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn characters(&self) -> usize {
        self.characters
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_piece_leaves_the_reply_as_it_was() {
        let mut buffer = ReplyBuffer::default();
        buffer.push("safe\n").expect("safe provider fragment");
        let accepted = buffer.as_str().to_owned();
        assert_eq!(
            buffer.push("unsafe\u{0000}"),
            Err(ReplyError::UnsupportedControlCharacter)
        );
        assert_eq!(buffer.as_str(), accepted);
        buffer
            .push(&"x".repeat(MAX_REPLY_CHARACTERS - buffer.characters()))
            .expect("fill exact provider boundary");
        assert_eq!(buffer.characters(), MAX_REPLY_CHARACTERS);
        assert_eq!(buffer.push("🙂"), Err(ReplyError::TextTooLong));
        assert_eq!(buffer.characters(), MAX_REPLY_CHARACTERS);
    }
}
