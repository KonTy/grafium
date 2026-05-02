#[derive(Debug, Clone)]
pub enum QueryNode {
    Page(String),
    Text(String),
    And(Vec<QueryNode>),
    Or(Vec<QueryNode>),
    Property(String, String),
    TaskState(String),
    Scheduled(DateFilter),
    Deadline(DateFilter),
    CreatedSince(u32),
    UpdatedSince(u32),
}

#[derive(Debug, Clone)]
pub enum DateFilter {
    Today,
    Before(String),
    After(String),
    On(String),
}
