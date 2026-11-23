use crate::asana::Task;
use chrono::{Duration, NaiveDate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    InProgress,
    UpNext,
    Done,
    /// Everything else; hidden from the Today view, visible in My Tasks.
    Later,
}

impl Bucket {
    pub const TODAY_ORDER: [Bucket; 3] = [Bucket::InProgress, Bucket::UpNext, Bucket::Done];

    pub fn title(self) -> &'static str {
        match self {
            Bucket::InProgress => "In progress",
            Bucket::UpNext => "Up next",
            Bucket::Done => "Done since yesterday",
            Bucket::Later => "Later",
        }
    }
}

const UP_NEXT_DAYS: i64 = 10;

/// In progress = your My Tasks "Today" section. Up next = due within
/// `UP_NEXT_DAYS`, overdue included. Open Rex sessions don't affect placement.
pub fn classify(task: &Task, today: NaiveDate) -> Bucket {
    if task.completed {
        return Bucket::Done;
    }
    if task.section() == "Today" {
        return Bucket::InProgress;
    }
    let horizon = today + Duration::days(UP_NEXT_DAYS);
    match task.due_on {
        Some(due) if due <= horizon => Bucket::UpNext,
        _ => Bucket::Later,
    }
}
