//! Reusable UI components — Toggle, Slider, Modal, SegmentedControl, TextField, stat_row.

#[allow(dead_code)]
pub mod modal;
pub mod segmented;
pub mod slider;
pub mod stat_row;
pub mod text_field;
pub mod toggle;

pub use segmented::{SegmentedControl, SegmentedMode};
pub use slider::Slider;
#[allow(unused_imports)]
pub use stat_row::{stat_row, stat_row_with_id};
pub use text_field::TextField;
pub use toggle::Toggle;
