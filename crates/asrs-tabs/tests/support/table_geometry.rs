//! A Table tab's layout in a real-geometry Blink run: each read takes the next recorded value
//! and checks it against [`TableLayout`]. Column widths depend on the recording machine's
//! fonts, so horizontal positions are taken from the record.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_model::source_list::Rect;
use asrs_parity::geometry::ScriptedReads;
use asrs_tabs::table::layout::TableLayout;
use asrs_tabs::table::{TableGeometry, TableRows};

pub const CONTAINER: &str = "div.data-table-container:2";
const HEADER: &str = "div.data-table-container:2>table.data-table:0>tbody:0>tr:0";

fn data_row(index: usize) -> String {
    format!(
        "div.data-table-container:2>table.data-table:0>tbody:0>tr:{}",
        index + 2
    )
}

fn header_cell(index: usize) -> String {
    format!("{HEADER}>th:{index}")
}

fn rect([x, y, width, height]: [f64; 4]) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

#[derive(Default)]
pub struct CheckedTable {
    pub reads: ScriptedReads,
    pub layout: TableLayout,
    pub checked: usize,
    pub mismatches: Vec<String>,
}

impl CheckedTable {
    fn check(&mut self, what: &str, model: &[f64], recorded: &[f64]) {
        self.checked += 1;
        if model != recorded {
            self.mismatches.push(format!(
                "{what}: the layout model has {model:?}, Blink {recorded:?}"
            ));
        }
    }
}

#[derive(Clone, Default)]
pub struct SharedTable(pub Rc<RefCell<CheckedTable>>);

impl TableGeometry for SharedTable {
    fn scroll_top(&mut self) -> f64 {
        let mut t = self.0.borrow_mut();
        let value = t.reads.read_f64("scrollTop", CONTAINER);
        let model = t.layout.scroll_top();
        t.check("scrollTop", &[model], &[value]);
        value
    }

    fn set_scroll_top(&mut self, value: f64) {
        let mut t = self.0.borrow_mut();
        t.reads.write_f64("scrollTop", CONTAINER, value);
        t.layout.set_scroll_top(value);
    }

    fn container_client_height(&mut self) -> f64 {
        let mut t = self.0.borrow_mut();
        let value = t.reads.read_f64("clientHeight", CONTAINER);
        let model = t.layout.container_client_height();
        t.check("container clientHeight", &[model], &[value]);
        value
    }

    fn header_client_height(&mut self) -> f64 {
        let mut t = self.0.borrow_mut();
        let value = t.reads.read_f64("clientHeight", HEADER);
        let model = t.layout.header_client_height();
        t.check("header clientHeight", &[model], &[value]);
        value
    }

    fn header_rect(&mut self) -> Rect {
        let mut t = self.0.borrow_mut();
        let value = rect(t.reads.read_rect(HEADER));
        let model = t.layout.header_rect();
        t.check(
            "header row",
            &[model.y, model.height],
            &[value.y, value.height],
        );
        value
    }

    fn data_row_rect(&mut self, index: usize, hidden: bool) -> Rect {
        let mut t = self.0.borrow_mut();
        let value = rect(t.reads.read_rect(&data_row(index)));
        let model = t.layout.data_row_rect(index, hidden);
        t.check(
            &format!("row {index}"),
            &[model.y, model.height],
            &[value.y, value.height],
        );
        value
    }

    fn container_rect(&mut self) -> Rect {
        let mut t = self.0.borrow_mut();
        let value = rect(t.reads.read_rect(CONTAINER));
        let model = t.layout.container_rect();
        t.check(
            "container",
            &[model.x, model.y, model.width, model.height],
            &[value.x, value.y, value.width, value.height],
        );
        value
    }

    fn header_cell_rect(&mut self, index: usize) -> Rect {
        let mut t = self.0.borrow_mut();
        let value = rect(t.reads.read_rect(&header_cell(index)));
        let model = t.layout.header_cell_rect(index);
        t.check(
            &format!("header cell {index}"),
            &[model.y, model.height],
            &[value.y, value.height],
        );
        if index == 0 {
            t.check("sticky header cell", &[model.x], &[value.x]);
        }
        value
    }

    fn header_key_client_width(&mut self, index: usize) -> f64 {
        let target = format!("{}>div.data-table-key-container:0", header_cell(index));
        self.0.borrow_mut().reads.read_f64("clientWidth", &target)
    }

    fn header_key_text_width(&mut self, index: usize) -> f64 {
        let target = format!(
            "{}>div.data-table-key-container:0>span:0",
            header_cell(index)
        );
        self.0.borrow_mut().reads.read_f64("offsetWidth", &target)
    }

    fn rows_changed(&mut self, rows: TableRows) {
        self.0.borrow_mut().layout.rows_changed(rows);
    }
}
