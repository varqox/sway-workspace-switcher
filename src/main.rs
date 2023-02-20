/// Server program that abstracts sway workspaces into layers of 2D grids and navigates through them

use serde_json::Value;
use std::os::unix::fs::FileTypeExt;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio_stream::wrappers::LinesStream;
use tokio_stream::StreamExt;

macro_rules! simple_display_and_debug {
    ($newtype:ident) => {
        impl std::fmt::Display for $newtype {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let $newtype(val) = self;
                f.write_fmt(format_args!("{val}"))
            }
        }
        impl std::fmt::Debug for $newtype {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let $newtype(val) = self;
                f.write_fmt(format_args!("{val}"))
            }
        }
    };
}

mod workspace {
    use crate::layer;
    use serde::Deserialize;

    #[derive(Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
    pub(crate) struct Id(pub u32);

    simple_display_and_debug!(Id);

    #[derive(Clone, Copy, Deserialize)]
    pub(crate) struct Num(pub i32);

    simple_display_and_debug!(Num);

    impl Num {
        pub(crate) fn to_coordinates(self) -> Option<Coordinates> {
            if self.0 < 100 {
                return None;
            }
            Some(Coordinates {
                row: self.0 as u32 / 10 % 10,
                col: self.0 as u32 % 10,
            })
        }

        pub(crate) fn to_name(self) -> String {
            let Some(coords) = self.to_coordinates() else {
                return self.to_string();
            };
            match coords {
                Coordinates { row: 0, col: 0 } => format!("{self}:⌜ "),
                Coordinates { row: 0, col: 1 } => format!("{self}:⌃"),
                Coordinates { row: 0, col: 2 } => format!("{self}: ⌝"),
                Coordinates { row: 1, col: 0 } => format!("{self}:◃  "),
                Coordinates { row: 1, col: 1 } => format!("{self}:⋄"),
                Coordinates { row: 1, col: 2 } => format!("{self}:  ▹"),
                Coordinates { row: 2, col: 0 } => format!("{self}:⌞ "),
                Coordinates { row: 2, col: 1 } => format!("{self}:⌄"),
                Coordinates { row: 2, col: 2 } => format!("{self}: ⌟"),
                _ => self.to_string(),
            }
        }

        pub(crate) fn to_layer_id(self) -> Option<layer::Id> {
            let layer_id = self.0 / 100;
            if layer_id <= 0 {
                return None;
            }
            Some(layer::Id(layer_id as u32))
        }
    }

    #[derive(Clone, Debug)]
    pub(crate) struct Coordinates {
        pub row: u32,
        pub col: u32,
    }

    #[derive(Debug)]
    pub(crate) struct Workspace {
        pub num: Num,
        pub name: String,
    }

    impl From<crate::swaymsg::get_workspaces::Workspace> for Workspace {
        fn from(workspace: crate::swaymsg::get_workspaces::Workspace) -> Self {
            Self {
                num: workspace.num,
                name: workspace.name,
            }
        }
    }
}

mod layer {
    use crate::workspace;

    pub(crate) const ROWS: u32 = 3;
    pub(crate) const COLUMNS: u32 = 3;

    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub(crate) struct Id(pub u32);

    simple_display_and_debug!(Id);

    impl Id {
        pub(crate) fn to_workspace_num(
            self,
            coordinates: workspace::Coordinates,
        ) -> workspace::Num {
            assert!(self.0 > 0);
            assert!(coordinates.row < 10);
            assert!(coordinates.col < 10);
            workspace::Num((self.0 * 100 + coordinates.row * 10 + coordinates.col) as i32)
        }
    }

    #[derive(Debug)]
    pub(crate) struct Layer {
        pub workspaces_num: usize,
        // During transition from an empty workspace to a new layer, we need history of
        // 3 workspaces to retain information about the last focused nonempty workspace.
        // Invariant: first element is Some(_); before Some(_) there is no None.
        pub last_focused_3_workspaces: Vec<workspace::Id>,
    }
}

mod swaymsg {
    use crate::workspace;
    use tokio::process::Command;

    pub(crate) mod subscribe {
        pub(crate) mod workspace {
            use crate::workspace;
            use serde::Deserialize;

            #[derive(Deserialize, Debug)]
            pub(crate) struct Workspace {
                pub id: workspace::Id,
                pub num: workspace::Num,
                pub name: String,
            }

            pub(crate) mod events {
                use super::Workspace;
                use serde::Deserialize;

                #[derive(Deserialize, Debug)]
                pub(crate) struct Init {
                    pub current: Workspace,
                }

                #[derive(Deserialize, Debug)]
                pub(crate) struct Empty {
                    pub current: Workspace,
                }

                #[derive(Deserialize, Debug)]
                pub(crate) struct Focus {
                    pub current: Workspace,
                }

                #[derive(Deserialize, Debug)]
                pub(crate) struct Rename {
                    pub current: Workspace,
                }

                #[derive(Deserialize, Debug)]
                pub(crate) struct Reload {}
            }
        }
    }

    pub(crate) mod get_workspaces {
        use crate::workspace;
        use serde::Deserialize;

        #[derive(Deserialize, Debug)]
        pub(crate) struct Workspace {
            pub id: workspace::Id,
            pub num: workspace::Num,
            pub name: String,
            pub focused: bool,
            pub visible: bool,
        }
    }

    pub(crate) async fn get_workspaces() -> Vec<get_workspaces::Workspace> {
        serde_json::from_slice(
            &Command::new("swaymsg")
                .args(["--type", "get_workspaces"])
                .output()
                .await
                .unwrap()
                .stdout,
        )
        .unwrap()
    }

    pub(crate) async fn rename_workspace(old_name: &str, new_name: &str) {
        tracing::info!("Renaming workspace {old_name:?} to {new_name:?}");
        let status = Command::new("swaymsg")
            .args(["rename", "workspace", old_name, "to", new_name])
            .status()
            .await
            .unwrap();
        assert!(status.success());
    }

    pub(crate) async fn switch_to_workspace(num: workspace::Num) {
        tracing::info!("Switch to workspace with number {num}");
        let status = Command::new("swaymsg")
            .args(["workspace", "number", &num.to_name()])
            .status()
            .await
            .unwrap();
        assert!(status.success());
    }

    pub(crate) async fn move_window_to_workspace(num: workspace::Num) {
        tracing::info!("Move focused window to workspace with number {num}");
        let status = Command::new("swaymsg")
            .args([
                "move",
                "window",
                "to",
                "workspace",
                "number",
                &num.to_name(),
            ])
            .status()
            .await
            .unwrap();
        assert!(status.success());
    }
}

mod state {
    use crate::{
        layer::{self, Layer},
        swaymsg, workspace,
    };
    use std::collections::{BTreeMap, HashMap};

    pub(crate) struct State {
        workspaces: HashMap<workspace::Id, workspace::Workspace>,
        layers: BTreeMap<layer::Id, Layer>,
        focused: Option<workspace::Id>,
        last_layer_id: layer::Id,
    }

    mod from_impl {
        use crate::workspace;

        #[derive(Debug)]
        pub(super) struct Layer {
            pub workspaces_num: usize,
            pub visible_workspace_id: Option<workspace::Id>,
            pub any_unvisible_workspace_id: Option<workspace::Id>,
        }
    }

    impl State {
        async fn create_workspace(
            &mut self,
            mut workspace: swaymsg::subscribe::workspace::Workspace,
        ) {
            let layer_id = match workspace.num.to_layer_id() {
                Some(layer_id) => layer_id,
                None => {
                    self.last_layer_id.0 += 1;
                    workspace.num = self
                        .last_layer_id
                        .to_workspace_num(workspace::Coordinates { row: 0, col: 0 });
                    let old_name = std::mem::replace(&mut workspace.name, workspace.num.to_name());
                    swaymsg::rename_workspace(&old_name, &workspace.name).await;
                    self.last_layer_id
                }
            };
            self.last_layer_id = self.last_layer_id.max(layer_id);
            self.layers
                .entry(layer_id)
                .and_modify(|layer| {
                    layer.workspaces_num += 1;
                })
                .or_insert_with(|| {
                    tracing::info!("Creating layer {layer_id}");

                    let mut last_focused_3_workspaces = Vec::with_capacity(3);
                    last_focused_3_workspaces.push(workspace.id);
                    tracing::trace!("last_focused_3_workspaces: [{}]", workspace.num);

                    Layer {
                        workspaces_num: 1,
                        last_focused_3_workspaces,
                    }
                });
            tracing::info!(
                "Creating workspace number {} (name: {})",
                workspace.num,
                workspace.name
            );
            self.workspaces
                .entry(workspace.id)
                .or_insert(workspace::Workspace {
                    num: workspace.num,
                    name: workspace.name,
                });
        }

        pub(crate) async fn from(
            got_workspaces: Vec<crate::swaymsg::get_workspaces::Workspace>,
        ) -> Self {
            let mut last_layer_id = got_workspaces
                .iter()
                .filter_map(|workspace| workspace.num.to_layer_id())
                .fold(layer::Id(0), std::cmp::max);

            for workspace in &got_workspaces {
                tracing::info!(
                    "Found workspace {} (name: {})",
                    workspace.num,
                    workspace.name
                );
            }
            tracing::trace!("last_layer_id: {last_layer_id}");

            let mut workspaces = HashMap::new();
            let mut layers = HashMap::new();
            let mut focused = None;
            for mut workspace in got_workspaces {
                let layer_id = match workspace.num.to_layer_id() {
                    Some(layer_id) => layer_id,
                    None => {
                        last_layer_id.0 += 1;
                        workspace.num = last_layer_id
                            .to_workspace_num(workspace::Coordinates { row: 0, col: 0 });
                        tracing::trace!("new workspace.num = {}", workspace.num);
                        let old_name =
                            std::mem::replace(&mut workspace.name, workspace.num.to_name());
                        swaymsg::rename_workspace(&old_name, &workspace.name).await;
                        last_layer_id
                    }
                };

                let layer = layers.entry(layer_id).or_insert_with(|| from_impl::Layer {
                    workspaces_num: 0,
                    visible_workspace_id: workspace.visible.then_some(workspace.id),
                    any_unvisible_workspace_id: (!workspace.visible).then_some(workspace.id),
                });
                layer.workspaces_num += 1;

                if workspace.visible {
                    layer.visible_workspace_id = Some(workspace.id);

                    if workspace.focused {
                        assert!(focused.is_none());
                        focused = Some(workspace.id);
                    }
                }

                workspaces.insert(workspace.id, workspace::Workspace::from(workspace));
            }

            if let Some(workspace_id) = focused {
                let workspace = workspaces.get(&workspace_id).unwrap();
                tracing::info!(
                    "Focused workspace {} (name: {})",
                    workspace.num,
                    workspace.name
                );
            }

            let layers = layers
                .into_iter()
                .map(|(layer_id, layer)| {
                    (
                        layer_id,
                        Layer {
                            workspaces_num: layer.workspaces_num,
                            last_focused_3_workspaces: {
                                let mut last_focused_3_workspaces = Vec::with_capacity(3);
                                last_focused_3_workspaces.extend(
                                    #[allow(clippy::filter_map_identity)]
                                    [layer.visible_workspace_id, layer.any_unvisible_workspace_id]
                                        .into_iter()
                                        .filter_map(|x| x),
                                );
                                assert!(!last_focused_3_workspaces.is_empty());
                                tracing::trace!(
                                    "Layer {layer_id}: last_focused_3_workspaces: {:?}",
                                    last_focused_3_workspaces
                                        .iter()
                                        .map(|workspace_id| workspaces
                                            .get(workspace_id)
                                            .unwrap()
                                            .num)
                                        .collect::<Vec<_>>()
                                );
                                last_focused_3_workspaces
                            },
                        },
                    )
                })
                .collect();

            Self {
                workspaces,
                layers,
                focused,
                last_layer_id,
            }
        }

        pub(crate) async fn handle_event(&mut self, event: Event) {
            use Event::*;
            match event {
                Create(workspace) => {
                    self.create_workspace(workspace).await;
                }
                Delete(workspace_id) => {
                    let workspace = self.workspaces.remove(&workspace_id).unwrap();
                    tracing::info!(
                        "Removed workspace {} (name: {})",
                        workspace.num,
                        workspace.name
                    );
                    let layer_id = workspace.num.to_layer_id().unwrap();
                    let layer = self.layers.get_mut(&layer_id).unwrap();
                    layer
                        .last_focused_3_workspaces
                        .retain(|wid| wid != &workspace_id);
                    layer.workspaces_num -= 1;
                    if layer.workspaces_num == 0 {
                        tracing::info!("Removing layer {layer_id}");
                        self.layers.remove(&layer_id).unwrap();
                    } else {
                        tracing::trace!(
                            "Layer {layer_id}: last_focused_3_workspaces: {:?}",
                            layer
                                .last_focused_3_workspaces
                                .iter()
                                .map(|workspace_id| self.workspaces.get(workspace_id).unwrap().num)
                                .collect::<Vec<_>>()
                        );
                    }
                }
                Focused(workspace_id) => {
                    let workspace = self.workspaces.get(&workspace_id).unwrap();
                    tracing::info!(
                        "Focused workspace {} (name: {})",
                        workspace.num,
                        workspace.name
                    );
                    let layer_id = workspace.num.to_layer_id().unwrap();
                    let last_focused_3_workspaces = &mut self
                        .layers
                        .get_mut(&layer_id)
                        .unwrap()
                        .last_focused_3_workspaces;
                    last_focused_3_workspaces.retain(|wid| wid != &workspace_id);
                    if last_focused_3_workspaces.len() > 2 {
                        last_focused_3_workspaces.pop();
                    }
                    last_focused_3_workspaces.insert(0, workspace_id);

                    tracing::trace!(
                        "Layer {layer_id}: last_focused_3_workspaces: {:?}",
                        last_focused_3_workspaces
                            .iter()
                            .map(|workspace_id| self.workspaces.get(workspace_id).unwrap().num)
                            .collect::<Vec<_>>()
                    );

                    self.focused = Some(workspace_id);
                }
                Rename {
                    id,
                    new_num,
                    new_name,
                } => {
                    let workspace = self.workspaces.get_mut(&id).unwrap();
                    tracing::info!(
                        "Renamed workspace {} -> {new_num} (name: {} -> {new_name})",
                        workspace.num,
                        workspace.name,
                    );
                    workspace.num = new_num;
                    workspace.name = new_name;
                }
            }
        }

        async fn impl_switch_carry_workspace(&mut self, carry: bool, kind: SwitchKind) {
            let old_workspace = {
                let Some(workspace_id) = self.focused else {
                    return tracing::error!("Cannot switch workspace if none is focused");
                };
                self.workspaces.get(&workspace_id).unwrap()
            };
            let old_layer_id = old_workspace.num.to_layer_id().unwrap();
            let old_coordinates = old_workspace.num.to_coordinates().unwrap();

            use SwitchKind::*;
            let workspace_num = match kind {
                Right => old_layer_id.to_workspace_num(workspace::Coordinates {
                    col: (old_coordinates.col + 1) % layer::COLUMNS,
                    ..old_coordinates
                }),
                Left => old_layer_id.to_workspace_num(workspace::Coordinates {
                    col: (old_coordinates.col + layer::COLUMNS - 1) % layer::COLUMNS,
                    ..old_coordinates
                }),
                Down => old_layer_id.to_workspace_num(workspace::Coordinates {
                    row: (old_coordinates.row + 1) % layer::ROWS,
                    ..old_coordinates
                }),
                Up => old_layer_id.to_workspace_num(workspace::Coordinates {
                    row: (old_coordinates.row + layer::ROWS - 1) % layer::ROWS,
                    ..old_coordinates
                }),
                NextLayer => {
                    self.workspaces
                        .get(
                            &self
                                .layers
                                .range((
                                    std::ops::Bound::Excluded(old_layer_id),
                                    std::ops::Bound::Unbounded,
                                ))
                                .next()
                                .or_else(|| self.layers.first_key_value())
                                .unwrap()
                                .1
                                .last_focused_3_workspaces[0],
                        )
                        .unwrap()
                        .num
                }
                NewLayer => layer::Id(self.last_layer_id.0 + 1)
                    .to_workspace_num(workspace::Coordinates { row: 0, col: 0 }),
            };
            if carry {
                swaymsg::move_window_to_workspace(workspace_num).await;
            }
            // self.focused = Some(workspace_num);
            swaymsg::switch_to_workspace(workspace_num).await;
        }

        pub(crate) async fn switch_workspace(&mut self, kind: SwitchKind) {
            tracing::info!("Command: switch workspace to {:?}", kind);
            self.impl_switch_carry_workspace(false, kind).await
        }

        pub(crate) async fn carry_to_workspace(&mut self, kind: SwitchKind) {
            tracing::info!("Command: carry and switch to workspace {:?}", kind);
            self.impl_switch_carry_workspace(true, kind).await
        }
    }

    pub(crate) enum Event {
        Create(swaymsg::subscribe::workspace::Workspace),
        Delete(workspace::Id),
        Focused(workspace::Id),
        Rename {
            id: workspace::Id,
            new_num: workspace::Num,
            new_name: String,
        },
    }

    #[derive(Debug)]
    pub(crate) enum SwitchKind {
        Right,
        Left,
        Up,
        Down,
        NextLayer,
        NewLayer,
    }
}

async fn create_command_pipe() -> std::io::Result<(tokio::fs::File, tokio::fs::File)> {
    let mut pipe_path = std::env::current_exe()?;
    pipe_path.set_file_name(format!(
        "{}.pipe",
        pipe_path.file_name().unwrap().to_str().unwrap()
    ));
    let pipe_path = pipe_path;
    tracing::info!("Command pipe path: {:?}", pipe_path);

    // Create pipe
    match tokio::fs::metadata(&pipe_path).await {
        Ok(metadata) => {
            // Remove path if it is not a pipe
            if !metadata.file_type().is_fifo() {
                match std::fs::remove_file(&pipe_path) {
                    Ok(()) => (),
                    Err(err) => match err.kind() {
                        std::io::ErrorKind::NotFound => (),
                        _ => panic!("remove_file() error: {err}"),
                    },
                }
                unix_named_pipe::create(&pipe_path, None)?;
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            unix_named_pipe::create(&pipe_path, None)?;
        }
        Err(err) => panic!("{err}"),
    }

    let _tmp_reader = unix_named_pipe::open_read(&pipe_path)?;
    let _tmp_writer = unix_named_pipe::open_write(&pipe_path)?;

    let reader = tokio::fs::File::open(&pipe_path).await?;
    let writer = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&pipe_path)
        .await?;
    Ok((reader, writer))
}

async fn handle_sway_events<T: tokio_stream::Stream<Item = Value> + Unpin>(
    state_mutex: Arc<Mutex<state::State>>,
    mut json_event_stream: T,
) {
    loop {
        let value = json_event_stream.next().await.unwrap();
        let event = value.as_object().unwrap()["change"].as_str().unwrap();
        tracing::trace!("Event: {event}");
        let mut state = state_mutex.lock().await;
        use state::Event::*;
        use swaymsg::subscribe::workspace::events::*;
        match event {
            "init" => {
                let init: Init = serde_json::from_value(value).unwrap();
                state.handle_event(Create(init.current)).await;
            }
            "empty" => {
                let empty: Empty = serde_json::from_value(value).unwrap();
                state.handle_event(Delete(empty.current.id)).await;
            }
            "focus" => {
                let focus: Focus = serde_json::from_value(value).unwrap();
                state.handle_event(Focused(focus.current.id)).await;
            }
            "move" => {}
            "rename" => {
                let rename: swaymsg::subscribe::workspace::events::Rename =
                    serde_json::from_value(value).unwrap();
                state
                    .handle_event(state::Event::Rename {
                        id: rename.current.id,
                        new_num: rename.current.num,
                        new_name: rename.current.name,
                    })
                    .await;
            }
            "urgent" => {}
            "reload" => {}
            unknown => {
                panic!("Unknown event: {unknown}");
            }
        }
    }
}

async fn handle_commands<T: tokio_stream::Stream<Item = String> + Unpin>(
    state_mutex: Arc<Mutex<state::State>>,
    mut command_stream: T,
) {
    loop {
        let command = command_stream.next().await.unwrap();
        let mut state = state_mutex.lock().await;
        use state::SwitchKind::*;
        match command.as_str() {
            "go right" => state.switch_workspace(Right).await,
            "go left" => state.switch_workspace(Left).await,
            "go up" => state.switch_workspace(Up).await,
            "go down" => state.switch_workspace(Down).await,
            "go to next layer" => state.switch_workspace(NextLayer).await,
            "go to new layer" => state.switch_workspace(NewLayer).await,
            "carry right" => state.carry_to_workspace(Right).await,
            "carry left" => state.carry_to_workspace(Left).await,
            "carry up" => state.carry_to_workspace(Up).await,
            "carry down" => state.carry_to_workspace(Down).await,
            "carry to next layer" => state.carry_to_workspace(NextLayer).await,
            "carry to new layer" => state.carry_to_workspace(NewLayer).await,
            _ => tracing::error!("Unknown command: {command}"),
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Holding pipe writer open prevents reading EOF if an external pipe writer closes file descriptor
    // (Linux pipe semantics is that if the last writer closes, the readers receive a read error,
    // we want to prevent that in this program)
    let (command_pipe, _pipe_writer) = create_command_pipe().await.unwrap();
    let mut swaymsg_process = Command::new("swaymsg")
        .args(["--monitor", "--type", "subscribe", "[\"workspace\"]"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    let workspaces = swaymsg::get_workspaces().await;
    let state_mutex = Arc::new(Mutex::new(state::State::from(workspaces).await));

    let json_event_stream =
        LinesStream::new(BufReader::new(swaymsg_process.stdout.take().unwrap()).lines())
            .map(|line| -> Value { serde_json::from_str(&line.unwrap()).unwrap() });

    let command_stream =
        LinesStream::new(BufReader::new(command_pipe).lines()).map(|line| line.unwrap());

    tokio::spawn(handle_sway_events(state_mutex.clone(), json_event_stream));
    handle_commands(state_mutex, command_stream).await;
}
