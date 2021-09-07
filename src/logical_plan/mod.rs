mod create;
mod generator;
mod insert;
mod projection;
mod seq_scan;

pub use create::*;
pub use generator::*;
pub use insert::*;
pub use projection::*;
pub use seq_scan::*;

// LogicalPlan is an enumeration which record all necessary information of execution plan, which will be used by optimizer and executor.
#[derive(thiserror::Error, Debug, PartialEq)]
pub enum LogicalPlanError {
    #[error("invalid SQL")]
    InvalidSQL,
}

#[derive(Debug, PartialEq, Clone)]
pub enum LogicalPlan {
    Dummy,
    SeqScan(SeqScanLogicalPlan),
    Insert(InsertLogicalPlan),
    CreateTable(CreateTableLogicalPlan),
    Projection(ProjectionLogicalPlan),
}