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
    /// Every accepted command is appended to the journal, so an interrupted
    /// session can be replayed. Rejected commands are never recorded.
    #[cfg(feature = "store")]
    pub fn with_journal(mut self, journal: crate::store::Journal) -> Self {
        self.journal = Some(journal);
        self
    }
    /// Applies a command. With a journal attached the command is recorded
    /// only after the model has accepted it, so a replay never stops at a
    /// rejected edit. If recording fails the edit is rolled back and the
    /// journal error returned, keeping the model and the journal in step.
    pub fn apply(&mut self, command: Command) -> Result<()> {
        #[cfg(feature = "store")]
        let recorded = self.journal.is_some().then(|| command.clone());
        let inverse = command.apply(&mut self.model)?;
        #[cfg(feature = "store")]
        if let Some(recorded) = recorded {
            self.record(&recorded, &inverse)?;
        }
        self.undo.push(inverse);
        self.redo.clear();
        Ok(())
    }
    /// Returns false when there is nothing to undo. The entry stays on the
    /// stack until the undo has both applied and been journalled.
    pub fn undo(&mut self) -> Result<bool> {
        let Some(inverse) = self.undo.last().cloned() else {
            return Ok(false);
        };
        let forward = inverse.clone().apply(&mut self.model)?;
        #[cfg(feature = "store")]
        self.record(&inverse, &forward)?;
        self.undo.pop();
        self.redo.push(forward);
        Ok(true)
    }
    pub fn redo(&mut self) -> Result<bool> {
        let Some(forward) = self.redo.last().cloned() else {
            return Ok(false);
        };
        let inverse = forward.clone().apply(&mut self.model)?;
        #[cfg(feature = "store")]
        self.record(&forward, &inverse)?;
        self.redo.pop();
        self.undo.push(inverse);
        Ok(true)
    }
    /// Journals an accepted command; on failure applies `rollback` so the
    /// model returns to the state the journal describes.
    #[cfg(feature = "store")]
    fn record(&mut self, command: &Command, rollback: &Command) -> Result<()> {
        let Some(j) = &self.journal else {
            return Ok(());
        };
        if let Err(e) = j.append(command) {
            rollback
                .clone()
                .apply(&mut self.model)
                .expect("rollback of an accepted command");
            return Err(ModelError::Invalid(format!("journal: {e}")));
        }
        Ok(())
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
