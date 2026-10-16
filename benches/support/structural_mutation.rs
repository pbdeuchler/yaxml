#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaseKind {
    AppendText,
    AppendCdata,
    AppendComment,
    AppendPi,
    RemoveFirst,
    RemoveMiddle,
    RemoveLast,
    RemoveDeep,
    Churn,
}

impl CaseKind {
    pub(crate) const ALL: [Self; 9] = [
        Self::AppendText,
        Self::AppendCdata,
        Self::AppendComment,
        Self::AppendPi,
        Self::RemoveFirst,
        Self::RemoveMiddle,
        Self::RemoveLast,
        Self::RemoveDeep,
        Self::Churn,
    ];

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|case| case.label() == value)
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::AppendText => "append_text",
            Self::AppendCdata => "append_cdata",
            Self::AppendComment => "append_comment",
            Self::AppendPi => "append_pi",
            Self::RemoveFirst => "remove_first",
            Self::RemoveMiddle => "remove_middle",
            Self::RemoveLast => "remove_last",
            Self::RemoveDeep => "remove_deep",
            Self::Churn => "churn",
        }
    }

    pub(crate) const fn reference_model(self) -> &'static str {
        match self {
            Self::AppendText => "xmlNewDocText+xmlAddChild",
            Self::AppendCdata => "xmlNewCDataBlock+xmlAddChild",
            Self::AppendComment => "xmlNewDocComment+xmlAddChild",
            Self::AppendPi => "xmlNewDocPI+xmlAddChild",
            Self::RemoveFirst | Self::RemoveMiddle | Self::RemoveLast | Self::RemoveDeep => {
                "xmlUnlinkNode+xmlFreeNode"
            }
            Self::Churn => "xmlNewDocText+xmlAddChild+xmlUnlinkNode+xmlFreeNode",
        }
    }

    pub(crate) const fn is_removal(self) -> bool {
        matches!(
            self,
            Self::RemoveFirst | Self::RemoveMiddle | Self::RemoveLast | Self::RemoveDeep
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct OperationCounts {
    pub(crate) requested: usize,
    pub(crate) completed: usize,
    pub(crate) created_nodes: usize,
    pub(crate) removed_nodes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccountingError {
    Overflow,
    RemovedMoreThanLive,
}

impl OperationCounts {
    pub(crate) fn request(&mut self) -> Result<(), AccountingError> {
        self.requested = self
            .requested
            .checked_add(1)
            .ok_or(AccountingError::Overflow)?;
        Ok(())
    }

    pub(crate) fn complete_append(&mut self) -> Result<(), AccountingError> {
        let completed = self
            .completed
            .checked_add(1)
            .ok_or(AccountingError::Overflow)?;
        let created = self
            .created_nodes
            .checked_add(1)
            .ok_or(AccountingError::Overflow)?;
        self.completed = completed;
        self.created_nodes = created;
        Ok(())
    }

    pub(crate) fn complete_removal(&mut self, removed_nodes: usize) -> Result<(), AccountingError> {
        let completed = self
            .completed
            .checked_add(1)
            .ok_or(AccountingError::Overflow)?;
        let removed = self
            .removed_nodes
            .checked_add(removed_nodes)
            .ok_or(AccountingError::Overflow)?;
        self.completed = completed;
        self.removed_nodes = removed;
        Ok(())
    }

    pub(crate) fn final_live(self, initial_live: usize) -> Result<usize, AccountingError> {
        initial_live
            .checked_add(self.created_nodes)
            .ok_or(AccountingError::Overflow)?
            .checked_sub(self.removed_nodes)
            .ok_or(AccountingError::RemovedMoreThanLive)
    }
}

pub(crate) fn removal_order(length: usize, case: CaseKind) -> Vec<usize> {
    let mut remaining: Vec<_> = (0..length).collect();
    let mut order = Vec::with_capacity(length);
    while !remaining.is_empty() {
        let position = match case {
            CaseKind::RemoveFirst => 0,
            CaseKind::RemoveMiddle | CaseKind::RemoveDeep => remaining.len() / 2,
            CaseKind::RemoveLast => remaining.len() - 1,
            _ => panic!("removal order requires a removal case"),
        };
        order.push(remaining.remove(position));
    }
    order
}
