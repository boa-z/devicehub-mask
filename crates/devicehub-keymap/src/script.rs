use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use devicehub_core::{HARDWARE_BUTTON_NAMES, KeyMappingResolution};
use rhai::{AST, Array, Dynamic, Engine, EvalAltResult, ImmutableString, Scope};

const MAX_SOURCE_BYTES: usize = 16 * 1024;
const MAX_OPERATIONS: u64 = 10_000;
const MAX_VARIABLES: usize = 64;
const MAX_STRING_BYTES: usize = 2 * 1024;
const MAX_ARRAY_ITEMS: usize = 64;
const MAX_STATE_ITEMS: usize = 64;
const MAX_ACTIONS: usize = 256;
const MAX_WAIT_MS: i64 = 10_000;
const MAX_TIMELINE_MS: u64 = 60_000;
const DEFAULT_PRESS_MS: u64 = 30;

#[derive(Debug, Clone, PartialEq)]
pub enum ScriptAction {
    Touch {
        identity: u8,
        touching: bool,
        x: f32,
        y: f32,
    },
    KeyboardDown {
        usage: u64,
    },
    KeyboardUp {
        usage: u64,
    },
    ButtonDown {
        name: String,
    },
    ButtonUp {
        name: String,
    },
    Text {
        text: String,
    },
    EnterFps {
        mapping_id: String,
    },
    ExitFps,
    SetRawInput {
        enabled: bool,
    },
    CancelCast {
        mapping_id: String,
    },
    ReleaseCast,
    Log {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduledScriptAction {
    pub at: Duration,
    pub action: ScriptAction,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScriptPlan {
    pub duration: Duration,
    pub actions: Vec<ScheduledScriptAction>,
}

#[derive(Debug, Clone, PartialEq)]
enum ScriptValue {
    Integer(i64),
    Boolean(bool),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScriptState {
    values: BTreeMap<String, ScriptValue>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScriptContext {
    pub frame: KeyMappingResolution,
    pub cursor_x: u32,
    pub cursor_y: u32,
    pub raw_input: bool,
    pub fps_mode: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError(String);

impl ScriptError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ScriptError {}

#[derive(Clone)]
pub struct ScriptProgram {
    source: Arc<str>,
    ast: AST,
}

impl fmt::Debug for ScriptProgram {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScriptProgram")
            .field("source_bytes", &self.source.len())
            .finish_non_exhaustive()
    }
}

impl ScriptProgram {
    pub fn compile(source: &str) -> Result<Self, ScriptError> {
        if source.len() > MAX_SOURCE_BYTES {
            return Err(ScriptError::new(format!(
                "script exceeds the {MAX_SOURCE_BYTES} byte limit"
            )));
        }
        let engine = base_engine();
        let normalized = normalize_statement_endings(source);
        let ast = engine.compile(normalized).map_err(script_error)?;
        Ok(Self {
            source: Arc::from(source),
            ast,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.source.trim().is_empty()
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn plan(
        &self,
        context: ScriptContext,
        state: &mut ScriptState,
    ) -> Result<ScriptPlan, ScriptError> {
        if self.is_empty() {
            return Ok(ScriptPlan::default());
        }
        let planner = Arc::new(Mutex::new(Planner {
            context,
            state: state.clone(),
            ..Planner::default()
        }));
        let engine = execution_engine(Arc::clone(&planner));
        let mut scope = Scope::new();
        scope.push_constant("SCREEN_W", i64::from(context.frame.width));
        scope.push_constant("SCREEN_H", i64::from(context.frame.height));
        scope.push_constant("ORIGINAL_W", i64::from(context.frame.width));
        scope.push_constant("ORIGINAL_H", i64::from(context.frame.height));
        scope.push_constant("CURSOR_X", i64::from(context.cursor_x));
        scope.push_constant("CURSOR_Y", i64::from(context.cursor_y));
        scope.push_constant("RawInputFlag", context.raw_input);
        scope.push_constant("FpsModeFlag", context.fps_mode);
        let _ = engine
            .eval_ast_with_scope::<Dynamic>(&mut scope, &self.ast)
            .map_err(script_error)?;
        drop(engine);
        let planner = Arc::try_unwrap(planner)
            .map_err(|_| ScriptError::new("script planner is still in use"))?
            .into_inner()
            .map_err(|_| ScriptError::new("script planner lock was poisoned"))?;
        *state = planner.state;
        Ok(ScriptPlan {
            duration: Duration::from_millis(planner.cursor_ms),
            actions: planner.actions,
        })
    }
}

pub fn validate_script(source: &str) -> Result<(), ScriptError> {
    ScriptProgram::compile(source).map(|_| ())
}

#[derive(Default)]
struct Planner {
    context: ScriptContext,
    cursor_ms: u64,
    actions: Vec<ScheduledScriptAction>,
    state: ScriptState,
}

impl Default for ScriptContext {
    fn default() -> Self {
        Self {
            frame: KeyMappingResolution {
                width: 1,
                height: 1,
            },
            cursor_x: 0,
            cursor_y: 0,
            raw_input: false,
            fps_mode: false,
        }
    }
}

impl Planner {
    fn schedule(&mut self, action: ScriptAction) -> Result<(), String> {
        if self.actions.len() >= MAX_ACTIONS {
            return Err(format!("script exceeds the {MAX_ACTIONS} action limit"));
        }
        self.actions.push(ScheduledScriptAction {
            at: Duration::from_millis(self.cursor_ms),
            action,
        });
        Ok(())
    }

    fn wait(&mut self, milliseconds: i64) -> Result<(), String> {
        if !(0..=MAX_WAIT_MS).contains(&milliseconds) {
            return Err(format!(
                "wait must be between 0 and {MAX_WAIT_MS} milliseconds"
            ));
        }
        self.cursor_ms = self
            .cursor_ms
            .checked_add(milliseconds as u64)
            .filter(|value| *value <= MAX_TIMELINE_MS)
            .ok_or_else(|| format!("script timeline exceeds {MAX_TIMELINE_MS} milliseconds"))?;
        Ok(())
    }

    fn position(&self, x: i64, y: i64) -> Result<(f32, f32), String> {
        if x < 0
            || y < 0
            || x > i64::from(self.context.frame.width)
            || y > i64::from(self.context.frame.height)
        {
            return Err(format!(
                "touch position ({x}, {y}) is outside {}x{}",
                self.context.frame.width, self.context.frame.height
            ));
        }
        Ok((
            x as f32 / self.context.frame.width.max(1) as f32,
            y as f32 / self.context.frame.height.max(1) as f32,
        ))
    }

    fn touch(&mut self, pointer: i64, x: i64, y: i64, action: &str) -> Result<(), String> {
        let identity = pointer_id(pointer)?;
        let (x, y) = self.position(x, y)?;
        match action {
            "default" => {
                self.schedule(ScriptAction::Touch {
                    identity,
                    touching: true,
                    x,
                    y,
                })?;
                self.wait(DEFAULT_PRESS_MS as i64)?;
                self.schedule(ScriptAction::Touch {
                    identity,
                    touching: false,
                    x,
                    y,
                })
            }
            "down" | "move" => self.schedule(ScriptAction::Touch {
                identity,
                touching: true,
                x,
                y,
            }),
            "up" => self.schedule(ScriptAction::Touch {
                identity,
                touching: false,
                x,
                y,
            }),
            _ => Err("touch action must be default, down, move, or up".into()),
        }
    }

    fn send_key(&mut self, code: &str, action: &str) -> Result<(), String> {
        let usage = keyboard_usage(code)
            .ok_or_else(|| format!("unsupported KeyboardEvent.code: {code}"))?;
        match action {
            "default" => {
                self.schedule(ScriptAction::KeyboardDown { usage })?;
                self.wait(DEFAULT_PRESS_MS as i64)?;
                self.schedule(ScriptAction::KeyboardUp { usage })
            }
            "down" => self.schedule(ScriptAction::KeyboardDown { usage }),
            "up" => self.schedule(ScriptAction::KeyboardUp { usage }),
            _ => Err("key action must be default, down, or up".into()),
        }
    }

    fn send_button(&mut self, name: &str, action: &str) -> Result<(), String> {
        if !HARDWARE_BUTTON_NAMES.contains(&name) {
            return Err(format!("unknown hardware button: {name}"));
        }
        match action {
            "default" => {
                self.schedule(ScriptAction::ButtonDown { name: name.into() })?;
                self.wait(DEFAULT_PRESS_MS as i64)?;
                self.schedule(ScriptAction::ButtonUp { name: name.into() })
            }
            "down" => self.schedule(ScriptAction::ButtonDown { name: name.into() }),
            "up" => self.schedule(ScriptAction::ButtonUp { name: name.into() }),
            _ => Err("button action must be default, down, or up".into()),
        }
    }
}

type RhaiResult<T> = Result<T, Box<EvalAltResult>>;

fn base_engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .set_max_operations(MAX_OPERATIONS)
        .set_max_variables(MAX_VARIABLES)
        .set_max_call_levels(16)
        .set_max_expr_depths(32, 16)
        .set_max_string_size(MAX_STRING_BYTES)
        .set_max_array_size(MAX_ARRAY_ITEMS)
        .set_max_map_size(MAX_ARRAY_ITEMS)
        .disable_symbol("eval")
        .disable_symbol("import")
        .disable_symbol("export")
        .disable_symbol("fn");
    engine
}

fn execution_engine(planner: Arc<Mutex<Planner>>) -> Engine {
    let mut engine = base_engine();

    let output = Arc::clone(&planner);
    engine.on_print(move |message| {
        let _ = with_planner(&output, |planner| {
            planner.schedule(ScriptAction::Log {
                message: message.chars().take(512).collect(),
            })
        });
    });

    let wait = Arc::clone(&planner);
    engine.register_fn("wait", move |milliseconds: i64| -> RhaiResult<()> {
        with_planner(&wait, |planner| planner.wait(milliseconds)).map_err(rhai_error)
    });

    let tap = Arc::clone(&planner);
    engine.register_fn(
        "tap",
        move |pointer: i64, x: i64, y: i64| -> RhaiResult<()> {
            with_planner(&tap, |planner| planner.touch(pointer, x, y, "default"))
                .map_err(rhai_error)
        },
    );
    let tap_action = Arc::clone(&planner);
    engine.register_fn(
        "tap",
        move |pointer: i64, x: i64, y: i64, action: ImmutableString| -> RhaiResult<()> {
            with_planner(&tap_action, |planner| {
                planner.touch(pointer, x, y, action.as_str())
            })
            .map_err(rhai_error)
        },
    );

    register_swipe_functions(&mut engine, Arc::clone(&planner));
    register_key_functions(&mut engine, Arc::clone(&planner));
    register_state_functions(&mut engine, Arc::clone(&planner));
    register_mode_functions(&mut engine, Arc::clone(&planner));
    engine
}

fn register_swipe_functions(engine: &mut Engine, planner: Arc<Mutex<Planner>>) {
    let array_planner = Arc::clone(&planner);
    engine.register_fn(
        "swipe",
        move |pointer: i64, interval: i64, points: Array| -> RhaiResult<()> {
            let points = points
                .into_iter()
                .map(|point| {
                    let point = point
                        .try_cast::<Array>()
                        .ok_or_else(|| rhai_error("each swipe point must be [x, y]"))?;
                    if point.len() != 2 {
                        return Err(rhai_error("each swipe point must contain exactly x and y"));
                    }
                    let x = point[0]
                        .as_int()
                        .map_err(|_| rhai_error("swipe x must be an integer"))?;
                    let y = point[1]
                        .as_int()
                        .map_err(|_| rhai_error("swipe y must be an integer"))?;
                    Ok((x, y))
                })
                .collect::<RhaiResult<Vec<_>>>()?;
            plan_swipe(&array_planner, pointer, interval, &points).map_err(rhai_error)
        },
    );
    let pair_planner = planner;
    engine.register_fn(
        "swipe",
        move |pointer: i64, interval: i64, x1: i64, y1: i64, x2: i64, y2: i64| -> RhaiResult<()> {
            plan_swipe(&pair_planner, pointer, interval, &[(x1, y1), (x2, y2)]).map_err(rhai_error)
        },
    );
}

fn plan_swipe(
    planner: &Arc<Mutex<Planner>>,
    pointer: i64,
    interval: i64,
    points: &[(i64, i64)],
) -> Result<(), String> {
    if points.len() < 2 || points.len() > 32 {
        return Err("swipe requires between 2 and 32 points".into());
    }
    if !(1..=MAX_WAIT_MS).contains(&interval) {
        return Err(format!(
            "swipe interval must be between 1 and {MAX_WAIT_MS}"
        ));
    }
    with_planner(planner, |planner| {
        let identity = pointer_id(pointer)?;
        for (index, (x, y)) in points.iter().copied().enumerate() {
            let (x, y) = planner.position(x, y)?;
            if index > 0 {
                planner.wait(interval)?;
            }
            planner.schedule(ScriptAction::Touch {
                identity,
                touching: true,
                x,
                y,
            })?;
        }
        planner.wait(DEFAULT_PRESS_MS as i64)?;
        let (x, y) = planner.position(
            points.last().expect("point count checked").0,
            points.last().expect("point count checked").1,
        )?;
        planner.schedule(ScriptAction::Touch {
            identity,
            touching: false,
            x,
            y,
        })
    })
}

fn register_key_functions(engine: &mut Engine, planner: Arc<Mutex<Planner>>) {
    let default_key = Arc::clone(&planner);
    engine.register_fn("send_key", move |code: ImmutableString| -> RhaiResult<()> {
        with_planner(&default_key, |planner| {
            planner.send_key(code.as_str(), "default")
        })
        .map_err(rhai_error)
    });
    let action_key = Arc::clone(&planner);
    engine.register_fn(
        "send_key",
        move |code: ImmutableString, action: ImmutableString| -> RhaiResult<()> {
            with_planner(&action_key, |planner| {
                planner.send_key(code.as_str(), action.as_str())
            })
            .map_err(rhai_error)
        },
    );
    let default_button = Arc::clone(&planner);
    engine.register_fn(
        "send_button",
        move |name: ImmutableString| -> RhaiResult<()> {
            with_planner(&default_button, |planner| {
                planner.send_button(name.as_str(), "default")
            })
            .map_err(rhai_error)
        },
    );
    let action_button = Arc::clone(&planner);
    engine.register_fn(
        "send_button",
        move |name: ImmutableString, action: ImmutableString| -> RhaiResult<()> {
            with_planner(&action_button, |planner| {
                planner.send_button(name.as_str(), action.as_str())
            })
            .map_err(rhai_error)
        },
    );
    let text_planner = planner;
    engine.register_fn(
        "paste_text",
        move |text: ImmutableString| -> RhaiResult<()> {
            if text.is_empty() || text.len() > 512 || text.chars().count() > 128 {
                return Err(rhai_error(
                    "paste_text requires 1 to 128 characters and at most 512 bytes",
                ));
            }
            with_planner(&text_planner, |planner| {
                planner.schedule(ScriptAction::Text {
                    text: text.to_string(),
                })
            })
            .map_err(rhai_error)
        },
    );
}

fn register_state_functions(engine: &mut Engine, planner: Arc<Mutex<Planner>>) {
    let set_int = Arc::clone(&planner);
    engine.register_fn(
        "state_set",
        move |name: ImmutableString, value: i64| -> RhaiResult<()> {
            set_state(&set_int, name.as_str(), ScriptValue::Integer(value)).map_err(rhai_error)
        },
    );
    let set_bool = Arc::clone(&planner);
    engine.register_fn(
        "state_set",
        move |name: ImmutableString, value: bool| -> RhaiResult<()> {
            set_state(&set_bool, name.as_str(), ScriptValue::Boolean(value)).map_err(rhai_error)
        },
    );
    let set_string = Arc::clone(&planner);
    engine.register_fn(
        "state_set",
        move |name: ImmutableString, value: ImmutableString| -> RhaiResult<()> {
            set_state(
                &set_string,
                name.as_str(),
                ScriptValue::String(value.to_string()),
            )
            .map_err(rhai_error)
        },
    );
    let get_int = Arc::clone(&planner);
    engine.register_fn(
        "state_get",
        move |name: ImmutableString, fallback: i64| -> RhaiResult<i64> {
            with_planner(&get_int, |planner| {
                match planner.state.values.get(name.as_str()) {
                    Some(ScriptValue::Integer(value)) => Ok(*value),
                    Some(_) => Err(format!("state {} has a different type", name)),
                    None => Ok(fallback),
                }
            })
            .map_err(rhai_error)
        },
    );
    let get_bool = Arc::clone(&planner);
    engine.register_fn(
        "state_get",
        move |name: ImmutableString, fallback: bool| -> RhaiResult<bool> {
            with_planner(&get_bool, |planner| {
                match planner.state.values.get(name.as_str()) {
                    Some(ScriptValue::Boolean(value)) => Ok(*value),
                    Some(_) => Err(format!("state {} has a different type", name)),
                    None => Ok(fallback),
                }
            })
            .map_err(rhai_error)
        },
    );
    let get_string = Arc::clone(&planner);
    engine.register_fn(
        "state_get",
        move |name: ImmutableString, fallback: ImmutableString| -> RhaiResult<ImmutableString> {
            with_planner(&get_string, |planner| {
                match planner.state.values.get(name.as_str()) {
                    Some(ScriptValue::String(value)) => Ok(value.clone().into()),
                    Some(_) => Err(format!("state {} has a different type", name)),
                    None => Ok(fallback),
                }
            })
            .map_err(rhai_error)
        },
    );
    let has = Arc::clone(&planner);
    engine.register_fn("state_has", move |name: ImmutableString| -> bool {
        with_planner(&has, |planner| {
            Ok(planner.state.values.contains_key(name.as_str()))
        })
        .unwrap_or(false)
    });
    let delete = Arc::clone(&planner);
    engine.register_fn("state_delete", move |name: ImmutableString| -> bool {
        with_planner(&delete, |planner| {
            Ok(planner.state.values.remove(name.as_str()).is_some())
        })
        .unwrap_or(false)
    });
    let clear = planner;
    engine.register_fn("state_clear", move || {
        let _ = with_planner(&clear, |planner| {
            planner.state.values.clear();
            Ok(())
        });
    });
}

fn register_mode_functions(engine: &mut Engine, planner: Arc<Mutex<Planner>>) {
    let enter_fps = Arc::clone(&planner);
    engine.register_fn("enter_fps", move |id: ImmutableString| -> RhaiResult<()> {
        non_empty_id(id.as_str())?;
        with_planner(&enter_fps, |planner| {
            planner.schedule(ScriptAction::EnterFps {
                mapping_id: id.to_string(),
            })
        })
        .map_err(rhai_error)
    });
    let exit_fps = Arc::clone(&planner);
    engine.register_fn("exit_fps", move || -> RhaiResult<()> {
        with_planner(&exit_fps, |planner| planner.schedule(ScriptAction::ExitFps))
            .map_err(rhai_error)
    });
    let enter_raw = Arc::clone(&planner);
    engine.register_fn("enter_raw_input", move || -> RhaiResult<()> {
        with_planner(&enter_raw, |planner| {
            planner.schedule(ScriptAction::SetRawInput { enabled: true })
        })
        .map_err(rhai_error)
    });
    let exit_raw = Arc::clone(&planner);
    engine.register_fn("exit_raw_input", move || -> RhaiResult<()> {
        with_planner(&exit_raw, |planner| {
            planner.schedule(ScriptAction::SetRawInput { enabled: false })
        })
        .map_err(rhai_error)
    });
    let cancel = Arc::clone(&planner);
    engine.register_fn(
        "cancel_cast",
        move |id: ImmutableString| -> RhaiResult<()> {
            non_empty_id(id.as_str())?;
            with_planner(&cancel, |planner| {
                planner.schedule(ScriptAction::CancelCast {
                    mapping_id: id.to_string(),
                })
            })
            .map_err(rhai_error)
        },
    );
    let release = planner;
    engine.register_fn("release_cast", move || -> RhaiResult<()> {
        with_planner(&release, |planner| {
            planner.schedule(ScriptAction::ReleaseCast)
        })
        .map_err(rhai_error)
    });
}

fn set_state(planner: &Arc<Mutex<Planner>>, name: &str, value: ScriptValue) -> Result<(), String> {
    if name.trim().is_empty() || name.len() > 64 {
        return Err("state name must contain 1 to 64 bytes".into());
    }
    if matches!(&value, ScriptValue::String(value) if value.len() > MAX_STRING_BYTES) {
        return Err(format!("state string exceeds {MAX_STRING_BYTES} bytes"));
    }
    with_planner(planner, |planner| {
        if !planner.state.values.contains_key(name) && planner.state.values.len() >= MAX_STATE_ITEMS
        {
            return Err(format!("script state exceeds {MAX_STATE_ITEMS} items"));
        }
        planner.state.values.insert(name.into(), value);
        Ok(())
    })
}

fn with_planner<T>(
    planner: &Arc<Mutex<Planner>>,
    operation: impl FnOnce(&mut Planner) -> Result<T, String>,
) -> Result<T, String> {
    let mut planner = lock_planner(planner)?;
    operation(&mut planner)
}

fn lock_planner(planner: &Arc<Mutex<Planner>>) -> Result<MutexGuard<'_, Planner>, String> {
    planner
        .lock()
        .map_err(|_| "script planner lock was poisoned".into())
}

fn pointer_id(pointer: i64) -> Result<u8, String> {
    u8::try_from(pointer)
        .ok()
        .filter(|identity| *identity < 5)
        .ok_or_else(|| "pointer id must be between 0 and 4".into())
}

fn non_empty_id(id: &str) -> RhaiResult<()> {
    if id.trim().is_empty() || id.len() > 128 {
        Err(rhai_error("mapping id must contain 1 to 128 bytes"))
    } else {
        Ok(())
    }
}

fn keyboard_usage(code: &str) -> Option<u64> {
    let bytes = code.as_bytes();
    if bytes.len() == 4 && code.starts_with("Key") && bytes[3].is_ascii_uppercase() {
        return Some(0x04 + u64::from(bytes[3] - b'A'));
    }
    if bytes.len() == 6 && code.starts_with("Digit") && bytes[5].is_ascii_digit() {
        return Some(if bytes[5] == b'0' {
            0x27
        } else {
            0x1e + u64::from(bytes[5] - b'1')
        });
    }
    if let Some(number) = code
        .strip_prefix('F')
        .and_then(|value| value.parse::<u64>().ok())
    {
        return match number {
            1..=12 => Some(0x3a + number - 1),
            13..=24 => Some(0x68 + number - 13),
            _ => None,
        };
    }
    Some(match code {
        "Enter" => 0x28,
        "Escape" => 0x29,
        "Backspace" => 0x2a,
        "Tab" => 0x2b,
        "Space" => 0x2c,
        "Minus" => 0x2d,
        "Equal" => 0x2e,
        "BracketLeft" => 0x2f,
        "BracketRight" => 0x30,
        "Backslash" => 0x31,
        "Semicolon" => 0x33,
        "Quote" => 0x34,
        "Backquote" => 0x35,
        "Comma" => 0x36,
        "Period" => 0x37,
        "Slash" => 0x38,
        "CapsLock" => 0x39,
        "Insert" => 0x49,
        "Home" => 0x4a,
        "PageUp" => 0x4b,
        "Delete" => 0x4c,
        "End" => 0x4d,
        "PageDown" => 0x4e,
        "ArrowRight" => 0x4f,
        "ArrowLeft" => 0x50,
        "ArrowDown" => 0x51,
        "ArrowUp" => 0x52,
        "ControlLeft" => 0xe0,
        "ShiftLeft" => 0xe1,
        "AltLeft" => 0xe2,
        "MetaLeft" => 0xe3,
        "ControlRight" => 0xe4,
        "ShiftRight" => 0xe5,
        "AltRight" => 0xe6,
        "MetaRight" => 0xe7,
        _ => return None,
    })
}

fn rhai_error(message: impl Into<String>) -> Box<EvalAltResult> {
    message.into().into()
}

fn script_error(error: impl fmt::Display) -> ScriptError {
    ScriptError::new(error.to_string())
}

fn normalize_statement_endings(source: &str) -> String {
    let mut normalized = String::with_capacity(source.len() + source.lines().count());
    let mut paren_depth = 0_u32;
    let mut bracket_depth = 0_u32;
    let mut in_block_comment = false;
    for line in source.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let mut in_string = false;
        let mut escaped = false;
        let mut comment_at = None;
        let bytes = body.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if in_block_comment {
                if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    in_block_comment = false;
                    index += 2;
                    continue;
                }
                index += 1;
                continue;
            }
            if in_string {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    in_string = false;
                }
                index += 1;
                continue;
            }
            if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
                comment_at = Some(index);
                break;
            }
            if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
                in_block_comment = true;
                index += 2;
                continue;
            }
            match byte {
                b'"' => in_string = true,
                b'(' => paren_depth = paren_depth.saturating_add(1),
                b')' => paren_depth = paren_depth.saturating_sub(1),
                b'[' => bracket_depth = bracket_depth.saturating_add(1),
                b']' => bracket_depth = bracket_depth.saturating_sub(1),
                _ => {}
            }
            index += 1;
        }
        let (code, comment) = comment_at.map_or((body, ""), |index| body.split_at(index));
        let trimmed = code.trim_end();
        let needs_semicolon = paren_depth == 0
            && bracket_depth == 0
            && !trimmed.is_empty()
            && trimmed
                .as_bytes()
                .last()
                .is_some_and(|last| !b";{},+-*/%&|=!<>:".contains(last));
        normalized.push_str(trimmed);
        if needs_semicolon {
            normalized.push(';');
        }
        normalized.push_str(comment);
        if line.ends_with('\n') {
            normalized.push('\n');
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ScriptContext {
        ScriptContext {
            frame: KeyMappingResolution {
                width: 1000,
                height: 500,
            },
            cursor_x: 500,
            cursor_y: 250,
            raw_input: false,
            fps_mode: true,
        }
    }

    #[test]
    fn plans_touch_wait_keyboard_and_text_on_virtual_time() {
        let program = ScriptProgram::compile(
            r#"
                tap(0, SCREEN_W / 2, SCREEN_H / 2)
                wait(20)
                send_key("Space")
                paste_text("hello")
            "#,
        )
        .unwrap();
        let plan = program
            .plan(context(), &mut ScriptState::default())
            .unwrap();

        assert_eq!(plan.duration, Duration::from_millis(80));
        assert_eq!(plan.actions.len(), 5);
        assert_eq!(plan.actions[0].at, Duration::ZERO);
        assert_eq!(plan.actions[1].at, Duration::from_millis(30));
        assert_eq!(plan.actions[2].at, Duration::from_millis(50));
        assert_eq!(plan.actions[3].at, Duration::from_millis(80));
        assert!(matches!(plan.actions[4].action, ScriptAction::Text { .. }));
    }

    #[test]
    fn preserves_typed_state_between_lifecycle_programs() {
        let pressed =
            ScriptProgram::compile("state_set(\"count\", state_get(\"count\", 0) + 1)").unwrap();
        let released =
            ScriptProgram::compile("if state_get(\"count\", 0) == 1 { tap(1, 100, 100); }")
                .unwrap();
        let mut state = ScriptState::default();

        pressed.plan(context(), &mut state).unwrap();
        let plan = released.plan(context(), &mut state).unwrap();

        assert_eq!(plan.actions.len(), 2);
    }

    #[test]
    fn bounds_loops_timeline_state_and_touch_ids() {
        let looped = ScriptProgram::compile("while true { let x = 1 + 1; }").unwrap();
        assert!(looped.plan(context(), &mut ScriptState::default()).is_err());

        let wait = ScriptProgram::compile("wait(10001)").unwrap();
        assert!(wait.plan(context(), &mut ScriptState::default()).is_err());

        let pointer = ScriptProgram::compile("tap(5, 10, 10)").unwrap();
        assert!(
            pointer
                .plan(context(), &mut ScriptState::default())
                .is_err()
        );
    }

    #[test]
    fn disables_dynamic_evaluation_and_imports() {
        assert!(ScriptProgram::compile("eval(\"40 + 2\")").is_err());
        assert!(ScriptProgram::compile("import \"module\" as value;").is_err());
        assert!(ScriptProgram::compile("fn action() { wait(1); } action();").is_err());
    }

    #[test]
    fn optional_line_semicolons_preserve_multiline_calls_and_comments() {
        let program = ScriptProgram::compile(
            "// comment\nlet x = (\n  40 + 2\n)\nif x == 42 {\n tap(0, 10, 10) // touch\n}\n",
        )
        .unwrap();
        let plan = program
            .plan(context(), &mut ScriptState::default())
            .unwrap();
        assert_eq!(plan.actions.len(), 2);
    }
}
