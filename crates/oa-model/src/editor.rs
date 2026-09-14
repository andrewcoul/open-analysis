//! Undo and redo on top of inverse-returning commands, with optional
//! journalling for crash recovery.
use crate::{command::*, model::Model};

pub struct Editor {
    pub model: Model,
    undo: Vec<Command>,
    redo: Vec<Command>,
    #[cfg(feature = "store")]
    journal: Option<crate::store::Journal>,
}
impl Editor {
    pub fn new(model: Model) -> Self {
        Self {
            model,
            undo: vec![],
            redo: vec![],
            #[cfg(feature = "store")]
            journal: None,
        }
    }
    /// Every applied command is appended to the journal before it is applied,
    /// so an interrupted session can be replayed.
    #[cfg(feature = "store")]
    pub fn with_journal(mut self, journal: crate::store::Journal) -> Self {
        self.journal = Some(journal);
        self
    }
    pub fn apply(&mut self, command: Command) -> Result<()> {
        #[cfg(feature = "store")]
        if let Some(j) = &self.journal {
            j.append(&command)
                .map_err(|e| ModelError::Invalid(format!("journal: {e}")))?;
        }
        let inverse = command.apply(&mut self.model)?;
        self.undo.push(inverse);
        self.redo.clear();
        Ok(())
    }
    /// Returns false when there is nothing to undo.
    pub fn undo(&mut self) -> Result<bool> {
        let Some(inverse) = self.undo.pop() else {
            return Ok(false);
        };
        #[cfg(feature = "store")]
        if let Some(j) = &self.journal {
            j.append(&inverse)
                .map_err(|e| ModelError::Invalid(format!("journal: {e}")))?;
        }
        let forward = inverse.apply(&mut self.model)?;
        self.redo.push(forward);
        Ok(true)
    }
    pub fn redo(&mut self) -> Result<bool> {
        let Some(forward) = self.redo.pop() else {
            return Ok(false);
        };
        #[cfg(feature = "store")]
        if let Some(j) = &self.journal {
            j.append(&forward)
                .map_err(|e| ModelError::Invalid(format!("journal: {e}")))?;
        }
        let inverse = forward.apply(&mut self.model)?;
        self.undo.push(inverse);
        Ok(true)
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
