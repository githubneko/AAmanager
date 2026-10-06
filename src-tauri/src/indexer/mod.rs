//! 索引器：后台分析任务队列与工作线程。

pub mod queue;
pub mod worker;

pub use worker::Worker;
