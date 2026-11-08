use nsh_core::prelude::*;

use crate::ui;

pub fn page() -> impl IntoElement {
    view().flex_1().child(ui::page_header("DNS", vec![]))
}
