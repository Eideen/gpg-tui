use crate::gpg::context::GpgContext;
use crate::gpg::key::GpgKey;
use crate::widget::row::RowItem;
use crate::widget::table::StatefulTable;
use anyhow::Result;
use std::cmp;
use std::convert::TryInto;
use tui::backend::Backend;
use tui::layout::{Constraint, Rect};
use tui::style::{Modifier, Style};
use tui::terminal::Frame;
use tui::text::Text;
use tui::widgets::{Block, Borders, Row, Table};

/// Threshold value (width) for minimizing.
const TABLE_MIN_THRESHOLD: u16 = 100;
/// Length of keys row in maximized mode.
const KEYS_ROW_MAX_LENGTH: u16 = 55;
/// Length of keys row in minimized mode.
const KEYS_ROW_MIN_LENGTH: u16 = 31;

/// Main application.
///
/// It operates the TUI via rendering the widgets
/// and updating the application state.
pub struct App {
	/// Is app running?
	pub running: bool,
	/// Is app minimized?
	pub minimized: bool,
	/// List of public keys.
	pub key_list: StatefulTable<GpgKey>,
}

impl App {
	/// Constructs a new instance of `App`.
	pub fn new() -> Result<Self> {
		Ok(Self {
			running: true,
			minimized: false,
			key_list: StatefulTable::with_items(GpgContext::new()?.get_keys()?),
		})
	}

	/// Reset the application state.
	pub fn refresh(&mut self) {
		self.key_list.state.select(Some(0));
		self.key_list.reset_scroll();
		self.minimized = false;
	}

	/// Renders the user interface.
	pub fn render<B: Backend>(&mut self, frame: &mut Frame<'_, B>) {
		let rect = frame.size();
		self.minimized = rect.width < TABLE_MIN_THRESHOLD;
		self.render_key_list(frame, rect);
	}

	/// Renders the list of public keys.
	fn render_key_list<B: Backend>(
		&mut self,
		frame: &mut Frame<'_, B>,
		rect: Rect,
	) {
		let max_row_height = rect.height.checked_sub(4).unwrap_or(rect.height);
		let max_row_width = rect
			.width
			.checked_sub(
				if self.minimized {
					KEYS_ROW_MIN_LENGTH
				} else {
					KEYS_ROW_MAX_LENGTH
				} + 5,
			)
			.unwrap_or(rect.width);
		frame.render_stateful_widget(
			Table::new(self.key_list.items.iter().map(|key| {
				let keys_row = RowItem::new(
					key.get_subkey_info(self.minimized),
					None,
					max_row_height,
					self.key_list.scroll,
				);
				let users_row = RowItem::new(
					key.get_user_info(self.minimized),
					Some(max_row_width),
					max_row_height,
					self.key_list.scroll,
				);
				Row::new(vec![
					Text::from(keys_row.get()),
					Text::from(users_row.get()),
				])
				.height(
					cmp::max(keys_row.len(), users_row.len())
						.try_into()
						.unwrap_or(1),
				)
				.bottom_margin(1)
				.style(Style::default())
			}))
			.header(
				Row::new(vec!["Key", "User"])
					.style(Style::default())
					.bottom_margin(1),
			)
			.block(Block::default().title("Table").borders(Borders::ALL))
			.style(Style::default())
			.highlight_style(Style::default().add_modifier(Modifier::BOLD))
			.widths(&[
				Constraint::Min(if self.minimized {
					KEYS_ROW_MIN_LENGTH
				} else {
					KEYS_ROW_MAX_LENGTH
				}),
				Constraint::Percentage(100),
			])
			.column_spacing(1),
			rect,
			&mut self.key_list.state,
		);
	}
}
