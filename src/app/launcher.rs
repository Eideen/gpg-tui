use crate::app::banner::Banner;
use crate::app::clipboard::CopyType;
use crate::app::command::Command;
use crate::app::keys::{KeyBinding, KEY_BINDINGS};
use crate::app::mode::Mode;
use crate::app::prompt::{OutputType, Prompt, COMMAND_PREFIX, SEARCH_PREFIX};
use crate::app::state::State;
use crate::app::style;
use crate::app::tab::Tab;
use crate::args::Args;
use crate::gpg::context::GpgContext;
use crate::gpg::key::{GpgKey, KeyDetail, KeyType};
use crate::widget::list::StatefulList;
use crate::widget::row::{RowItem, ScrollDirection};
use crate::widget::style::Color as WidgetColor;
use crate::widget::table::{StatefulTable, TableState};
use anyhow::{anyhow, Error as AnyhowError, Result};
use colorsys::Rgb;
use copypasta_ext::prelude::ClipboardProvider;
use copypasta_ext::x11_fork::ClipboardContext;
use std::cmp;
use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::path::Path;
use std::process::Command as OsCommand;
use std::str;
use std::str::FromStr;
use tui::backend::Backend;
use tui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use tui::style::{Color, Modifier, Style};
use tui::terminal::Frame;
use tui::text::{Span, Spans, Text};
use tui::widgets::{
	Block, Borders, Clear, List, ListItem, Paragraph, Row, Table, Wrap,
};
use unicode_width::UnicodeWidthStr;

/// Lengths of keys row in minimized/maximized mode.
const KEYS_ROW_LENGTH: (u16, u16) = (31, 55);
/// Max duration of prompt messages.
const MESSAGE_DURATION: u128 = 1750;

/// Main application.
///
/// It operates the TUI via rendering the widgets
/// and updating the application state.
pub struct App<'a> {
	/// Application state.
	pub state: State,
	/// Application mode.
	pub mode: Mode,
	/// Prompt manager.
	pub prompt: Prompt,
	/// Current tab.
	pub tab: Tab,
	/// Content of the options menu.
	pub options: StatefulList<Command>,
	/// Content of the key bindings list.
	pub key_bindings: StatefulList<KeyBinding<'a>>,
	/// Public/secret keys.
	pub keys: HashMap<KeyType, Vec<GpgKey>>,
	/// Table of public/secret keys.
	pub keys_table: StatefulTable<GpgKey>,
	/// States of the keys table.
	pub keys_table_states: HashMap<KeyType, TableState>,
	/// Level of detail to show for keys table.
	pub keys_table_detail: KeyDetail,
	/// Bottom margin value of the keys table.
	pub keys_table_margin: u16,
	/// Clipboard context.
	pub clipboard: Option<ClipboardContext>,
	/// GPGME context.
	pub gpgme: &'a mut GpgContext,
}

impl<'a> App<'a> {
	/// Constructs a new instance of `App`.
	pub fn new(gpgme: &'a mut GpgContext, args: &'a Args) -> Result<Self> {
		let keys = gpgme.get_all_keys()?;
		let keys_table = StatefulTable::with_items(
			keys.get(&KeyType::Public)
				.expect("failed to get public keys")
				.to_vec(),
		);
		Ok(Self {
			state: State::from(args),
			mode: Mode::Normal,
			prompt: Prompt::default(),
			tab: Tab::Keys(KeyType::Public),
			options: StatefulList::with_items(Vec::new()),
			key_bindings: StatefulList::with_items(KEY_BINDINGS.to_vec()),
			keys,
			keys_table,
			keys_table_states: HashMap::new(),
			keys_table_detail: KeyDetail::Minimum,
			keys_table_margin: 1,
			clipboard: match ClipboardContext::new() {
				Ok(clipboard) => Some(clipboard),
				Err(e) => {
					println!("failed to initialize clipboard: {:?}", e);
					None
				}
			},
			gpgme,
		})
	}

	/// Resets the application state.
	pub fn refresh(&mut self) -> Result<()> {
		self.state.refresh();
		self.mode = Mode::Normal;
		self.prompt.clear();
		self.options.state.select(Some(0));
		self.keys = self.gpgme.get_all_keys()?;
		self.keys_table_states.clear();
		self.keys_table_detail = KeyDetail::Minimum;
		self.keys_table_margin = 1;
		match self.tab {
			Tab::Keys(key_type) => {
				self.keys_table = StatefulTable::with_items(
					self.keys
						.get(&key_type)
						.unwrap_or_else(|| {
							panic!("failed to get {} keys", key_type)
						})
						.to_vec(),
				)
			}
			Tab::Help => {}
		};
		Ok(())
	}

	/// Handles the tick event of the application.
	///
	/// It is currently used to flush the prompt messages.
	pub fn tick(&mut self) {
		if let Some(clock) = self.prompt.clock {
			if clock.elapsed().as_millis() > MESSAGE_DURATION
				&& self.prompt.command.is_none()
			{
				self.prompt.clear()
			}
		}
	}

	/// Runs the given command which is used to specify
	/// the widget to render or action to perform.
	pub fn run_command(&mut self, command: Command) -> Result<()> {
		let mut show_options = false;
		if let Command::Confirm(ref cmd) = command {
			self.prompt.set_command(*cmd.clone())
		} else if self.prompt.command.is_some() {
			self.prompt.clear();
		}
		match command {
			Command::ShowHelp => {
				self.tab = Tab::Help;
				if self.key_bindings.state.selected().is_none() {
					self.key_bindings.state.select(Some(0));
				}
			}
			Command::ShowOutput(output_type, message) => {
				self.prompt.set_output((output_type, message))
			}
			Command::ShowOptions => {
				let prev_selection = self.options.state.selected();
				let prev_item_count = self.options.items.len();
				self.options = StatefulList::with_items(match self.tab {
					Tab::Keys(key_type) => {
						let selected_key = &self
							.keys_table
							.selected()
							.expect("invalid selection");
						vec![
							Command::None,
							Command::ShowHelp,
							Command::Refresh,
							Command::RefreshKeys,
							Command::Set(
								String::from("prompt"),
								String::from(":import "),
							),
							Command::Set(
								String::from("prompt"),
								String::from(":receive "),
							),
							Command::ExportKeys(
								key_type,
								vec![selected_key.get_id()],
							),
							Command::ExportKeys(key_type, Vec::new()),
							Command::Confirm(Box::new(Command::DeleteKey(
								key_type,
								selected_key.get_id(),
							))),
							Command::Confirm(Box::new(Command::SendKey(
								selected_key.get_id(),
							))),
							Command::EditKey(selected_key.get_id()),
							Command::SignKey(selected_key.get_id()),
							Command::GenerateKey,
							Command::Set(
								String::from("armor"),
								(!self.gpgme.config.armor).to_string(),
							),
							Command::Copy(CopyType::Key),
							Command::Copy(CopyType::KeyId),
							Command::Copy(CopyType::KeyFingerprint),
							Command::Copy(CopyType::KeyUserId),
							Command::Copy(CopyType::TableRow(1)),
							Command::Copy(CopyType::TableRow(2)),
							Command::Paste,
							Command::ToggleDetail(false),
							Command::ToggleDetail(true),
							Command::Set(
								String::from("margin"),
								String::from(if self.keys_table_margin == 1 {
									"0"
								} else {
									"1"
								}),
							),
							Command::Set(
								String::from("minimized"),
								(!self.state.minimized).to_string(),
							),
							Command::Set(
								String::from("colored"),
								(!self.state.colored).to_string(),
							),
							if self.mode == Mode::Visual {
								Command::SwitchMode(Mode::Normal)
							} else {
								Command::SwitchMode(Mode::Visual)
							},
							Command::Quit,
						]
					}
					Tab::Help => {
						vec![
							Command::None,
							Command::ListKeys(KeyType::Public),
							Command::ListKeys(KeyType::Secret),
							if self.mode == Mode::Visual {
								Command::SwitchMode(Mode::Normal)
							} else {
								Command::SwitchMode(Mode::Visual)
							},
							Command::Refresh,
							Command::Quit,
						]
					}
				});
				if prev_item_count == 0
					|| self.options.items.len() == prev_item_count
				{
					self.options.state.select(prev_selection.or(Some(0)));
				} else {
					self.options.state.select(Some(0));
				}
				show_options = true;
			}
			Command::ListKeys(key_type) => {
				if let Tab::Keys(previous_key_type) = self.tab {
					self.keys_table_states.insert(
						previous_key_type,
						self.keys_table.state.clone(),
					);
					self.keys.insert(
						previous_key_type,
						self.keys_table.default_items.clone(),
					);
				}
				self.keys_table = StatefulTable::with_items(
					self.keys
						.get(&key_type)
						.unwrap_or_else(|| {
							panic!("failed to get {} keys", key_type)
						})
						.to_vec(),
				);
				if let Some(state) = self.keys_table_states.get(&key_type) {
					self.keys_table.state = state.clone();
				}
				self.tab = Tab::Keys(key_type);
			}
			Command::ImportKeys(keys, false) => {
				if keys.is_empty() {
					self.prompt.set_output((
						OutputType::Failure,
						String::from("no files given"),
					))
				} else {
					match self.gpgme.import_keys(keys) {
						Ok(key_count) => {
							self.refresh()?;
							self.prompt.set_output((
								OutputType::Success,
								format!("{} keys imported", key_count),
							))
						}
						Err(e) => self.prompt.set_output((
							OutputType::Failure,
							format!("import error: {}", e),
						)),
					}
				}
			}
			Command::ExportKeys(key_type, ref patterns) => {
				self.prompt.set_output(
					match self
						.gpgme
						.export_keys(key_type, Some(patterns.to_vec()))
					{
						Ok(path) => {
							(OutputType::Success, format!("export: {}", path))
						}
						Err(e) => (
							OutputType::Failure,
							format!("export error: {}", e),
						),
					},
				);
			}
			Command::DeleteKey(key_type, ref key_id) => {
				match self.gpgme.delete_key(key_type, key_id.to_string()) {
					Ok(_) => {
						self.refresh()?;
					}
					Err(e) => self.prompt.set_output((
						OutputType::Failure,
						format!("delete error: {}", e),
					)),
				}
			}
			Command::SendKey(key_id) => {
				self.prompt.set_output(match self.gpgme.send_key(key_id) {
					Ok(key_id) => (
						OutputType::Success,
						format!("key sent to the keyserver: 0x{}", key_id),
					),
					Err(e) => {
						(OutputType::Failure, format!("send error: {}", e))
					}
				});
			}
			Command::GenerateKey
			| Command::RefreshKeys
			| Command::EditKey(_)
			| Command::SignKey(_)
			| Command::ImportKeys(_, true) => {
				let mut os_command = OsCommand::new("gpg");
				let os_command = match command {
					Command::EditKey(key) => {
						os_command.arg("--edit-key").arg(&key)
					}
					Command::SignKey(key) => {
						if let Some(default_key) =
							&self.gpgme.config.default_key
						{
							os_command.arg("--default-key").arg(default_key);
						}
						os_command.arg("--sign-key").arg(&key)
					}
					Command::ImportKeys(keys, _) => {
						os_command.arg("--receive-keys").args(&keys)
					}
					Command::RefreshKeys => os_command.arg("--refresh-keys"),
					_ => os_command.arg("--full-gen-key"),
				};
				match os_command.spawn() {
					Ok(mut child) => {
						child.wait()?;
						self.refresh()?;
					}
					Err(e) => self.prompt.set_output((
						OutputType::Failure,
						format!("execution error: {}", e),
					)),
				}
			}
			Command::ToggleDetail(true) => {
				self.keys_table_detail.increase();
				for key in self.keys_table.items.iter_mut() {
					key.detail = self.keys_table_detail;
				}
				for key in self.keys_table.default_items.iter_mut() {
					key.detail = self.keys_table_detail;
				}
			}
			Command::ToggleDetail(false) => {
				if let Some(index) = self.keys_table.state.tui.selected() {
					if let Some(key) = self.keys_table.items.get_mut(index) {
						key.detail.increase()
					}
					if self.keys_table.items.len()
						== self.keys_table.default_items.len()
					{
						if let Some(key) =
							self.keys_table.default_items.get_mut(index)
						{
							key.detail.increase()
						}
					}
				}
			}
			Command::Scroll(direction, false) => match direction {
				ScrollDirection::Down(_) => {
					if self.state.show_options {
						self.options.next();
						show_options = true;
					} else if Tab::Help == self.tab {
						self.key_bindings.next();
					} else {
						self.keys_table.next();
					}
				}
				ScrollDirection::Up(_) => {
					if self.state.show_options {
						self.options.previous();
						show_options = true;
					} else if Tab::Help == self.tab {
						self.key_bindings.previous();
					} else {
						self.keys_table.previous();
					}
				}
				ScrollDirection::Top => {
					if self.state.show_options {
						self.options.state.select(Some(0));
						show_options = true;
					} else if Tab::Help == self.tab {
						self.key_bindings.state.select(Some(0));
					} else {
						self.keys_table.state.tui.select(Some(0));
					}
				}
				ScrollDirection::Bottom => {
					if self.state.show_options {
						self.options.state.select(Some(
							self.options
								.items
								.len()
								.checked_sub(1)
								.unwrap_or_default(),
						));
						show_options = true;
					} else if Tab::Help == self.tab {
						self.key_bindings
							.state
							.select(Some(KEY_BINDINGS.len() - 1));
					} else {
						self.keys_table.state.tui.select(Some(
							self.keys_table
								.items
								.len()
								.checked_sub(1)
								.unwrap_or_default(),
						));
					}
				}
				_ => {}
			},
			Command::Scroll(direction, true) => {
				self.keys_table.scroll_row(direction);
			}
			Command::Set(option, value) => {
				if option == *"prompt"
					&& (value.starts_with(COMMAND_PREFIX)
						| value.starts_with(SEARCH_PREFIX))
				{
					self.prompt.clear();
					self.prompt.text = value;
				} else {
					self.prompt.set_output(match option.as_str() {
						"output" => {
							let path = Path::new(&value);
							if path.exists() {
								self.gpgme.config.output_dir =
									path.to_path_buf();
								(
									OutputType::Success,
									format!(
										"output directory: {:?}",
										self.gpgme.config.output_dir
									),
								)
							} else {
								(
									OutputType::Failure,
									String::from("path does not exist"),
								)
							}
						}
						"mode" => {
							if let Ok(mode) = Mode::from_str(&value) {
								self.mode = mode;
								(
									OutputType::Success,
									format!(
										"mode: {}",
										format!("{:?}", mode).to_lowercase()
									),
								)
							} else {
								(
									OutputType::Failure,
									String::from("invalid mode"),
								)
							}
						}
						"armor" => {
							if let Ok(value) = FromStr::from_str(&value) {
								self.gpgme.config.armor = value;
								self.gpgme.apply_config();
								(
									OutputType::Success,
									format!("armor: {}", value),
								)
							} else {
								(
									OutputType::Failure,
									String::from(
										"usage: set armor <true/false>",
									),
								)
							}
						}
						"minimized" => {
							self.state.minimize_threshold = 0;
							self.state.minimized =
								FromStr::from_str(&value).unwrap_or_default();
							(
								OutputType::Success,
								format!("minimized: {}", self.state.minimized),
							)
						}
						"minimize" => {
							self.state.minimize_threshold =
								value.parse().unwrap_or_default();
							(
								OutputType::Success,
								format!(
									"minimize threshold: {}",
									self.state.minimize_threshold
								),
							)
						}
						"detail" => {
							if let Ok(detail_level) =
								KeyDetail::from_str(&value)
							{
								if let Some(index) =
									self.keys_table.state.tui.selected()
								{
									if let Some(key) =
										self.keys_table.items.get_mut(index)
									{
										key.detail = detail_level;
									}
									if self.keys_table.items.len()
										== self.keys_table.default_items.len()
									{
										if let Some(key) = self
											.keys_table
											.default_items
											.get_mut(index)
										{
											key.detail = detail_level;
										}
									}
								}
								(
									OutputType::Success,
									format!("detail: {}", detail_level),
								)
							} else {
								(
									OutputType::Failure,
									String::from("usage: set detail <level>"),
								)
							}
						}
						"margin" => {
							self.keys_table_margin =
								value.parse().unwrap_or_default();
							(
								OutputType::Success,
								format!(
									"table margin: {}",
									self.keys_table_margin
								),
							)
						}
						"colored" => match value.parse() {
							Ok(colored) => {
								self.state.colored = colored;
								(
									OutputType::Success,
									format!("colored: {}", self.state.colored),
								)
							}
							Err(_) => (
								OutputType::Failure,
								String::from("usage: set colored <true/false>"),
							),
						},
						"color" => {
							self.state.color =
								WidgetColor::from(value.as_ref()).get();
							(
								OutputType::Success,
								format!(
									"color: {}",
									match self.state.color {
										Color::Rgb(r, g, b) =>
											Rgb::from((r, g, b)).to_hex_string(),
										_ => format!("{:?}", self.state.color)
											.to_lowercase(),
									}
								),
							)
						}
						_ => (
							OutputType::Failure,
							if !option.is_empty() {
								format!("unknown option: {}", option)
							} else {
								String::from("usage: set <option> <value>")
							},
						),
					})
				}
			}
			Command::Get(option) => {
				self.prompt.set_output(match option.as_str() {
					"output" => (
						OutputType::Success,
						format!(
							"output directory: {:?}",
							self.gpgme.config.output_dir.as_os_str()
						),
					),
					"mode" => (
						OutputType::Success,
						format!(
							"mode: {}",
							format!("{:?}", self.mode).to_lowercase()
						),
					),
					"armor" => (
						OutputType::Success,
						format!("armor: {}", self.gpgme.config.armor),
					),
					"minimized" => (
						OutputType::Success,
						format!("minimized: {}", self.state.minimized),
					),
					"minimize" => (
						OutputType::Success,
						format!(
							"minimize threshold: {}",
							self.state.minimize_threshold
						),
					),
					"detail" => {
						if let Some(index) =
							self.keys_table.state.tui.selected()
						{
							if let Some(key) = self.keys_table.items.get(index)
							{
								(
									OutputType::Success,
									format!("detail: {}", key.detail),
								)
							} else {
								(
									OutputType::Failure,
									String::from("invalid selection"),
								)
							}
						} else {
							(
								OutputType::Failure,
								String::from("unknown selection"),
							)
						}
					}
					"margin" => (
						OutputType::Success,
						format!("table margin: {}", self.keys_table_margin),
					),
					"colored" => (
						OutputType::Success,
						format!("colored: {}", self.state.colored),
					),
					"color" => (
						OutputType::Success,
						format!(
							"color: {}",
							match self.state.color {
								Color::Rgb(r, g, b) =>
									Rgb::from((r, g, b)).to_hex_string(),
								_ => format!("{:?}", self.state.color)
									.to_lowercase(),
							}
						),
					),
					_ => (
						OutputType::Failure,
						if !option.is_empty() {
							format!("unknown option: {}", option)
						} else {
							String::from("usage: get <option>")
						},
					),
				})
			}
			Command::SwitchMode(mode) => {
				if !(mode == Mode::Copy && self.keys_table.items.is_empty()) {
					self.mode = mode;
					self.prompt
						.set_output((OutputType::Action, mode.to_string()))
				}
			}
			Command::Copy(copy_type) => {
				let selected_key =
					&self.keys_table.selected().expect("invalid selection");
				let content = match copy_type {
					CopyType::TableRow(1) => Ok(selected_key
						.get_subkey_info(self.state.minimized)
						.join("\n")),
					CopyType::TableRow(2) => Ok(selected_key
						.get_user_info(self.state.minimized)
						.join("\n")),
					CopyType::TableRow(_) => Err(anyhow!("invalid row number")),
					CopyType::Key => {
						match self.gpgme.get_exported_keys(
							match self.tab {
								Tab::Keys(key_type) => key_type,
								_ => KeyType::Public,
							},
							Some(vec![selected_key.get_id()]),
						) {
							Ok(key) => str::from_utf8(&key)
								.map(|v| v.to_string())
								.map_err(AnyhowError::from),
							Err(e) => Err(e),
						}
					}
					CopyType::KeyId => Ok(selected_key.get_id()),
					CopyType::KeyFingerprint => {
						Ok(selected_key.get_fingerprint())
					}
					CopyType::KeyUserId => Ok(selected_key.get_user_id()),
				};
				match content {
					Ok(content) => {
						if let Some(clipboard) = self.clipboard.as_mut() {
							clipboard
								.set_contents(content)
								.expect("failed to set clipboard contents");
							self.prompt.set_output((
								OutputType::Success,
								format!("{} copied to clipboard", copy_type),
							));
						} else {
							self.prompt.set_output((
								OutputType::Failure,
								String::from("clipboard not available"),
							));
						}
					}
					Err(e) => {
						self.prompt.set_output((
							OutputType::Failure,
							format!("copy error: {}", e),
						));
					}
				}
				self.mode = Mode::Normal;
			}
			Command::Paste => {
				if let Some(clipboard) = self.clipboard.as_mut() {
					self.prompt.clear();
					self.prompt.text = format!(
						":{}",
						clipboard
							.get_contents()
							.expect("failed to get clipboard contents")
					);
				} else {
					self.prompt.set_output((
						OutputType::Failure,
						String::from("clipboard not available"),
					));
				}
			}
			Command::EnableInput => self.prompt.enable_command_input(),
			Command::Search(query) => {
				self.prompt.text = format!("/{}", query.unwrap_or_default());
				self.prompt.enable_search();
				self.keys_table.items = self.keys_table.default_items.clone();
			}
			Command::NextTab => {
				self.run_command(self.tab.next().get_command())?
			}
			Command::PreviousTab => {
				self.run_command(self.tab.previous().get_command())?
			}
			Command::Refresh => self.refresh()?,
			Command::Quit => self.state.running = false,
			Command::Confirm(_) | Command::None => {}
		}
		self.state.show_options = show_options;
		Ok(())
	}

	/// Renders all the widgets thus the user interface.
	pub fn render<B: Backend>(&mut self, frame: &mut Frame<'_, B>) {
		let rect = frame.size();
		if self.state.minimize_threshold != 0 {
			self.state.minimized = rect.width < self.state.minimize_threshold;
		}
		let chunks = Layout::default()
			.direction(Direction::Vertical)
			.constraints(
				[Constraint::Min(rect.height - 1), Constraint::Min(1)].as_ref(),
			)
			.split(rect);
		self.render_command_prompt(frame, chunks[1]);
		match self.tab {
			Tab::Keys(_) => self.render_keys_table(frame, chunks[0]),
			Tab::Help => self.render_help_tab(frame, chunks[0]),
		}
		if self.state.show_options {
			self.render_options_menu(frame, rect);
		}
	}

	/// Renders the command prompt. (widget)
	fn render_command_prompt<B: Backend>(
		&mut self,
		frame: &mut Frame<'_, B>,
		rect: Rect,
	) {
		frame.render_widget(
			Paragraph::new(Spans::from(if !self.prompt.text.is_empty() {
				vec![Span::raw(format!(
					"{}{}",
					self.prompt.output_type, self.prompt.text
				))]
			} else {
				let arrow_color = if self.state.colored {
					Color::LightBlue
				} else {
					Color::DarkGray
				};
				vec![
					Span::styled("< ", Style::default().fg(arrow_color)),
					match self.tab {
						Tab::Keys(key_type) => Span::raw(format!(
							"list {}{}",
							key_type,
							if !self.keys_table.items.is_empty() {
								format!(
									" ({}/{})",
									self.keys_table
										.state
										.tui
										.selected()
										.unwrap_or_default() + 1,
									self.keys_table.items.len()
								)
							} else {
								String::new()
							}
						)),
						Tab::Help => Span::raw("help"),
					},
					Span::styled(" >", Style::default().fg(arrow_color)),
				]
			}))
			.style(if self.state.colored {
				match self.prompt.output_type {
					OutputType::Success => Style::default()
						.fg(Color::LightGreen)
						.add_modifier(Modifier::BOLD),
					OutputType::Warning => Style::default()
						.fg(Color::LightYellow)
						.add_modifier(Modifier::BOLD),
					OutputType::Failure => Style::default()
						.fg(Color::LightRed)
						.add_modifier(Modifier::BOLD),
					OutputType::Action => {
						if self.state.colored {
							Style::default()
								.fg(Color::LightBlue)
								.add_modifier(Modifier::BOLD)
						} else {
							Style::default().add_modifier(Modifier::BOLD)
						}
					}
					OutputType::None => Style::default(),
				}
			} else if self.prompt.output_type != OutputType::None {
				Style::default().add_modifier(Modifier::BOLD)
			} else {
				Style::default()
			})
			.alignment(if !self.prompt.text.is_empty() {
				Alignment::Left
			} else {
				Alignment::Right
			})
			.wrap(Wrap { trim: false }),
			rect,
		);
		if self.prompt.is_enabled() {
			frame.set_cursor(
				rect.x + self.prompt.text.width() as u16,
				rect.y + 1,
			);
		}
	}

	/// Renders the help tab.
	fn render_help_tab<B: Backend>(
		&mut self,
		frame: &mut Frame<'_, B>,
		rect: Rect,
	) {
		frame.render_widget(
			Block::default()
				.borders(Borders::ALL)
				.border_style(Style::default().fg(Color::DarkGray)),
			rect,
		);
		let chunks = Layout::default()
			.direction(Direction::Horizontal)
			.margin(1)
			.constraints(
				[Constraint::Percentage(50), Constraint::Percentage(50)]
					.as_ref(),
			)
			.split(rect);
		{
			let description = self
				.key_bindings
				.selected()
				.map(|v| {
					v.get_description_text(
						Style::default()
							.fg(Color::DarkGray)
							.add_modifier(Modifier::ITALIC),
					)
				})
				.unwrap_or_default();
			let description_height = u16::try_from(
				self.key_bindings
					.selected()
					.map(|v| v.description.lines().count())
					.unwrap_or_default(),
			)
			.unwrap_or(1) + 2;
			let chunks = Layout::default()
				.direction(Direction::Vertical)
				.margin(1)
				.constraints(
					[
						Constraint::Min(
							chunks[0]
								.height
								.checked_sub(description_height)
								.unwrap_or_default(),
						),
						Constraint::Min(description_height),
					]
					.as_ref(),
				)
				.split(chunks[0]);
			frame.render_stateful_widget(
				List::new(
					self.key_bindings
						.items
						.iter()
						.enumerate()
						.map(|(i, v)| {
							v.as_list_item(
								self.state.colored,
								self.key_bindings.state.selected() == Some(i),
							)
						})
						.collect::<Vec<ListItem>>(),
				)
				.block(
					Block::default()
						.borders(Borders::RIGHT)
						.border_style(Style::default().fg(Color::DarkGray)),
				)
				.style(Style::default().fg(self.state.color))
				.highlight_style(if self.state.colored {
					Style::default().add_modifier(Modifier::BOLD)
				} else {
					Style::default()
						.fg(Color::Reset)
						.add_modifier(Modifier::BOLD)
				})
				.highlight_symbol("> "),
				chunks[0],
				&mut self.key_bindings.state,
			);
			frame.render_widget(
				Paragraph::new(description)
					.block(
						Block::default()
							.borders(Borders::RIGHT)
							.border_style(Style::default().fg(Color::DarkGray)),
					)
					.style(Style::default().fg(self.state.color))
					.alignment(Alignment::Left)
					.wrap(Wrap { trim: true }),
				chunks[1],
			);
		}
		{
			let information = match self.gpgme.config.get_info() {
				Ok(text) => text,
				Err(e) => e.to_string(),
			};
			let information_height =
				u16::try_from(information.lines().count()).unwrap_or(1);
			let chunks = Layout::default()
				.direction(Direction::Vertical)
				.margin(1)
				.constraints(
					[
						Constraint::Min(
							chunks[1]
								.height
								.checked_sub(information_height)
								.unwrap_or_default(),
						),
						Constraint::Min(information_height),
					]
					.as_ref(),
				)
				.split(chunks[1]);
			let banner = Banner::get(chunks[0]);
			frame.render_widget(
				Paragraph::new(if self.state.colored {
					style::get_colored_info(&banner, Color::Magenta)
				} else {
					Text::raw(banner)
				})
				.block(
					Block::default()
						.borders(Borders::BOTTOM)
						.border_style(Style::default().fg(Color::DarkGray)),
				)
				.style(Style::default().fg(self.state.color))
				.alignment(Alignment::Left)
				.wrap(Wrap { trim: false }),
				chunks[0],
			);
			frame.render_widget(
				Paragraph::new(if self.state.colored {
					style::get_colored_info(&information, Color::Cyan)
				} else {
					Text::raw(information)
				})
				.block(
					Block::default()
						.borders(Borders::NONE)
						.border_style(Style::default().fg(Color::DarkGray)),
				)
				.style(Style::default().fg(self.state.color))
				.alignment(Alignment::Left)
				.wrap(Wrap { trim: true }),
				chunks[1],
			);
		}
	}

	/// Renders the options menu.
	fn render_options_menu<B: Backend>(
		&mut self,
		frame: &mut Frame<'_, B>,
		rect: Rect,
	) {
		let items = self
			.options
			.items
			.iter()
			.map(|v| ListItem::new(Span::raw(v.to_string())))
			.collect::<Vec<ListItem>>();
		let (length_x, mut percent_y) = (38, 60);
		let text_height =
			items.iter().map(|v| v.height() as f32).sum::<f32>() + 3.;
		if rect.height.checked_sub(5).unwrap_or(rect.height) as f32
			> text_height
		{
			percent_y = ((text_height / rect.height as f32) * 100.) as u16;
		}
		let popup_layout = Layout::default()
			.direction(Direction::Vertical)
			.constraints(
				[
					Constraint::Percentage((100 - percent_y) / 2),
					Constraint::Percentage(percent_y),
					Constraint::Percentage((100 - percent_y) / 2),
				]
				.as_ref(),
			)
			.split(rect);
		let area = Layout::default()
			.direction(Direction::Horizontal)
			.constraints(
				[
					Constraint::Length(
						(popup_layout[1].width.checked_sub(length_x))
							.unwrap_or_default() / 2,
					),
					Constraint::Min(length_x),
					Constraint::Length(
						(popup_layout[1].width.checked_sub(length_x))
							.unwrap_or_default() / 2,
					),
				]
				.as_ref(),
			)
			.split(popup_layout[1])[1];
		frame.render_widget(Clear, area);
		frame.render_stateful_widget(
			List::new(items)
				.block(
					Block::default()
						.title("Options")
						.style(if self.state.colored {
							Style::default().fg(Color::LightBlue)
						} else {
							Style::default()
						})
						.borders(Borders::ALL),
				)
				.style(Style::default().fg(self.state.color))
				.highlight_style(
					Style::default()
						.fg(Color::Reset)
						.add_modifier(Modifier::BOLD),
				)
				.highlight_symbol("> "),
			area,
			&mut self.options.state,
		);
	}

	/// Renders the table of keys.
	fn render_keys_table<B: Backend>(
		&mut self,
		frame: &mut Frame<'_, B>,
		rect: Rect,
	) {
		frame.render_stateful_widget(
			Table::new(
				self.get_keys_table_rows(
					rect.width
						.checked_sub(
							if self.state.minimized {
								KEYS_ROW_LENGTH.0
							} else {
								KEYS_ROW_LENGTH.1
							} + 7,
						)
						.unwrap_or(rect.width),
					rect.height.checked_sub(2).unwrap_or(rect.height),
				),
			)
			.style(Style::default().fg(self.state.color))
			.highlight_style(if self.state.colored {
				Style::default().add_modifier(Modifier::BOLD)
			} else {
				Style::default()
					.fg(Color::Reset)
					.add_modifier(Modifier::BOLD)
			})
			.highlight_symbol("> ")
			.block(
				Block::default()
					.borders(Borders::ALL)
					.border_style(Style::default().fg(Color::DarkGray)),
			)
			.widths(&[
				Constraint::Min(if self.state.minimized {
					KEYS_ROW_LENGTH.0
				} else {
					KEYS_ROW_LENGTH.1
				}),
				Constraint::Percentage(100),
			])
			.column_spacing(1),
			rect,
			&mut self.keys_table.state.tui,
		);
	}

	/// Returns the rows for keys table.
	fn get_keys_table_rows(
		&mut self,
		max_width: u16,
		max_height: u16,
	) -> Vec<Row<'a>> {
		let mut rows = Vec::new();
		self.keys_table.items = self
			.keys_table
			.items
			.clone()
			.into_iter()
			.enumerate()
			.filter(|(i, key)| {
				let subkey_info = key.get_subkey_info(self.state.minimized);
				let user_info = key.get_user_info(self.state.minimized);
				if self.prompt.is_search_enabled() {
					let search_term =
						self.prompt.text.replacen("/", "", 1).to_lowercase();
					if !subkey_info
						.join("\n")
						.to_lowercase()
						.contains(&search_term) && !user_info
						.join("\n")
						.to_lowercase()
						.contains(&search_term)
					{
						return false;
					}
				}
				let keys_row = RowItem::new(
					subkey_info,
					None,
					max_height,
					self.keys_table.state.scroll,
				);
				let users_row = RowItem::new(
					user_info,
					Some(max_width),
					max_height,
					self.keys_table.state.scroll,
				);
				rows.push(
					Row::new(if self.state.colored {
						let highlighted =
							self.keys_table.state.tui.selected() == Some(*i);
						vec![
							style::get_colored_table_row(
								&keys_row.data,
								highlighted,
							),
							style::get_colored_table_row(
								&users_row.data,
								highlighted,
							),
						]
					} else {
						vec![
							Text::from(keys_row.data.join("\n")),
							Text::from(users_row.data.join("\n")),
						]
					})
					.height(
						cmp::max(keys_row.data.len(), users_row.data.len())
							.try_into()
							.unwrap_or(1),
					)
					.bottom_margin(self.keys_table_margin)
					.style(Style::default()),
				);
				true
			})
			.map(|(_, v)| v)
			.collect();
		rows
	}
}

#[cfg(feature = "gpg-tests")]
#[cfg(test)]
mod tests {
	use super::*;
	use crate::args::Args;
	use crate::gpg::config::GpgConfig;
	use pretty_assertions::assert_eq;
	use std::env;
	use tui::backend::TestBackend;
	use tui::buffer::Buffer;
	use tui::Terminal;
	#[test]
	fn test_app_launcher() -> Result<()> {
		env::set_var(
			"GNUPGHOME",
			dirs::cache_dir()
				.unwrap()
				.join(env!("CARGO_PKG_NAME"))
				.to_str()
				.unwrap(),
		);
		let args = Args::default();
		let config = GpgConfig::new(&args)?;
		let mut context = GpgContext::new(config)?;
		let mut app = App::new(&mut context, &args)?;
		let backend = TestBackend::new(70, 10);
		let mut terminal = Terminal::new(backend)?;
		terminal.draw(|f| app.render(f))?;
		let mut expected = Buffer::with_lines(vec![
			"┌────────────────────────────────────────────────────────────────────┐",
			format!(
				"│> [sc--] rsa3072/{} [u] test@example.org              │",
				app.gpgme.get_all_keys()?.get(&KeyType::Public).unwrap()[0]
					.get_id()
			)
			.replace("0x", "").as_ref(),
			"│                                                                    │",
			"│  [sc--] rsa4096/53F218C35C1DC8B1 [?] menyoki.cli@protonmail.com    │",
			"│                                                                    │",
			"│                                                                    │",
			"│                                                                    │",
			"│                                                                    │",
			"└────────────────────────────────────────────────────────────────────┘",
			"                                                    < list pub (1/2) >",
		]);
		assert_eq!(expected.area, terminal.backend().size().unwrap());
		for x in 0..expected.area().width {
			for y in 0..expected.area().height {
				expected
					.get_mut(x, y)
					.set_style(terminal.backend().buffer().get(x, y).style());
			}
		}
		terminal.backend().assert_buffer(&expected);
		Ok(())
	}
}
