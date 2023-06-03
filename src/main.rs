/// Server program that abstracts sway workspaces into layers of 2D grids and navigates through them
use serde_json::Value;
use std::io::IsTerminal;
use std::os::unix::fs::FileTypeExt;
use std::process::Stdio;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::Command;
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

    #[derive(Clone, Copy, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub(crate) struct Id(pub u32);

    simple_display_and_debug!(Id);

    #[derive(Clone, Copy, Deserialize, Eq, PartialEq, Hash)]
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
}

mod swaymsg {
    use crate::workspace;
    use tokio::process::Command;

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
            pub output: String,
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

    pub(crate) async fn move_current_workspace_to_output(output: &str) {
        tracing::info!("Moving current workspace to output {output:?}");
        let status = Command::new("swaymsg")
            .args(["move", "workspace", "to", "output", output])
            .status()
            .await
            .unwrap();
        assert!(status.success());
    }

    pub(crate) async fn switch_to_workspace(num: workspace::Num) {
        tracing::info!("Switch to workspace with number {num}");
        Command::new("swaymsg")
            .args(["workspace", "number", &num.to_name()])
            .status()
            .await
            .unwrap();
    }

    pub(crate) async fn move_window_to_workspace(num: workspace::Num) {
        tracing::info!("Move focused window to workspace with number {num}");
        Command::new("swaymsg")
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

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_ansi(std::io::stdout().is_terminal())
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

    let mut json_event_stream =
        LinesStream::new(BufReader::new(swaymsg_process.stdout.take().unwrap()).lines())
            .map(|line| -> Value { serde_json::from_str(&line.unwrap()).unwrap() });

    let mut command_stream =
        LinesStream::new(BufReader::new(command_pipe).lines()).map(|line| line.unwrap());

    let mut persistent_state = persistent_state::State::new();
    {
        let mut state = current_state::State::new(&persistent_state).await;
        tracing::trace!("Initial state: {state:#?}");
        state.repair().await;
        persistent_state.update(&state);
    }
    loop {
        tokio::select! {
            event = json_event_stream.next() => {
                handle_sway_event(event.unwrap(), &mut persistent_state).await;
            }
            command = command_stream.next() => {
                handle_command(command.unwrap(), &mut persistent_state).await;
            }
        }
    }
}

async fn handle_sway_event(
    _event: serde_json::Value,
    persistent_state: &mut persistent_state::State,
) {
    let mut state = current_state::State::new(persistent_state).await;
    state.repair().await;
    persistent_state.update(&state);
}

async fn handle_command(command: String, persistent_state: &mut persistent_state::State) {
    let mut state = current_state::State::new(persistent_state).await;
    state.repair().await;

    use SwitchKind::*;
    match command.as_str() {
        "go right" => state.switch_to_workspace(Right).await,
        "go left" => state.switch_to_workspace(Left).await,
        "go up" => state.switch_to_workspace(Up).await,
        "go down" => state.switch_to_workspace(Down).await,
        "go to next layer" => state.switch_to_workspace(NextLayer).await,
        "go to new layer" => state.switch_to_workspace(NewLayer).await,
        "carry right" => state.carry_to_workspace(Right).await,
        "carry left" => state.carry_to_workspace(Left).await,
        "carry up" => state.carry_to_workspace(Up).await,
        "carry down" => state.carry_to_workspace(Down).await,
        "carry to next layer" => state.carry_to_workspace(NextLayer).await,
        "carry to new layer" => state.carry_to_workspace(NewLayer).await,
        _ => tracing::error!("Unknown command: {command}"),
    }

    persistent_state.update(&state);
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

mod persistent_state {
    use crate::{current_state, layer, workspace};
    use std::collections::HashMap;

    #[derive(Debug)]
    pub(crate) struct Layer {
        pub last_focused_workspace_num: workspace::Num,
    }

    #[derive(Debug)]
    pub(crate) struct State {
        pub layers: HashMap<layer::Id, Layer>,
        pub next_layer_id: layer::Id,
    }

    impl State {
        pub(crate) fn new() -> Self {
            Self {
                layers: HashMap::new(),
                next_layer_id: layer::Id(1),
            }
        }

        pub(crate) fn update(&mut self, current_state: &current_state::State) {
            tracing::trace!("Updating persistent state with the current state: {current_state:#?}");
            self.next_layer_id = current_state.next_layer_id;
            // Remove layers that disappeared
            self.layers
                .retain(|layer_id, _| current_state.layers.contains_key(layer_id));
            // Update or add layers
            for (layer_id, layer) in &current_state.layers {
                self.layers.insert(
                    layer_id.clone(),
                    Layer {
                        last_focused_workspace_num: layer.last_focused_workspace_num,
                    },
                );
            }
        }
    }
}

mod current_state {
    use crate::{layer, persistent_state, swaymsg, workspace, SwitchKind};
    use std::{
        cmp::max,
        collections::{BTreeMap, HashMap, HashSet},
        mem,
    };

    #[derive(Debug)]
    pub(crate) struct State {
        pub workspaces: BTreeMap<workspace::Id, Workspace>,
        pub layers: BTreeMap<layer::Id, Layer>,
        pub next_layer_id: layer::Id,
        pub focused_workspace_id: Option<workspace::Id>,
    }

    impl State {
        pub(crate) async fn new(persistent_state: &persistent_state::State) -> Self {
            let span = tracing::trace_span!("Creating current state");
            let _enter = span.enter();

            let mut workspaces = BTreeMap::new();
            let mut layers = BTreeMap::new();
            let mut next_layer_id = persistent_state.next_layer_id;
            let mut focused_workspace_id = None;
            // let workspaces = swaymsg::get_workspaces().await;
            for workspace in swaymsg::get_workspaces().await {
                workspaces.insert(
                    workspace.id,
                    Workspace {
                        num: workspace.num,
                        name: workspace.name,
                        focused: workspace.focused,
                        visible: workspace.visible,
                        output: workspace.output,
                    },
                );

                if let Some(layer_id) = workspace.num.to_layer_id() {
                    layers
                        .entry(layer_id)
                        .and_modify(|layer: &mut Layer| {
                            layer.workspaces_num += 1;
                            if workspace.focused || workspace.visible {
                                layer.last_focused_workspace_num = workspace.num;
                            }
                        })
                        .or_insert_with(|| {
                            tracing::trace!("New layer: {layer_id}");
                            Layer {
                                workspaces_num: 1,
                                last_focused_workspace_num: if workspace.focused
                                    || workspace.visible
                                {
                                    workspace.num
                                } else {
                                    if let Some(layer) = persistent_state.layers.get(&layer_id) {
                                        layer.last_focused_workspace_num
                                    } else {
                                        workspace.num
                                    }
                                },
                            }
                        });

                    next_layer_id = max(next_layer_id, layer::Id(layer_id.0 + 1));
                }

                if workspace.focused {
                    focused_workspace_id = Some(workspace.id);
                }
            }
            tracing::trace!("Focused workspace id: {:?}", focused_workspace_id);

            Self {
                workspaces,
                layers,
                next_layer_id,
                focused_workspace_id,
            }
        }

        pub(crate) async fn repair(&mut self) {
            macro_rules! move_workspace_to_a_new_layer {
                ($workspace:ident) => {
                    let new_num = self
                        .next_layer_id
                        .to_workspace_num(workspace::Coordinates { row: 0, col: 0 });
                    Self::rename_workspace_impl(
                        $workspace,
                        new_num,
                        &mut self.layers,
                        &mut self.next_layer_id,
                    )
                    .await;
                };
            }
            // First fix names of the workspaces with assigned layer
            let mut taken_numbers = HashSet::new();
            for workspace in self.workspaces.values() {
                if workspace.num.to_layer_id().is_some()
                    && workspace.name == workspace.num.to_name()
                {
                    taken_numbers.insert(workspace.num);
                }
            }
            for workspace in self.workspaces.values_mut() {
                if workspace.num.to_layer_id().is_some()
                    && workspace.name != workspace.num.to_name()
                {
                    if taken_numbers.contains(&workspace.num) {
                        move_workspace_to_a_new_layer!(workspace);
                    } else {
                        Self::rename_workspace_impl(
                            workspace,
                            workspace.num,
                            &mut self.layers,
                            &mut self.next_layer_id,
                        )
                        .await;
                    }
                    taken_numbers.insert(workspace.num);
                }
            }
            // Assign number to the workspaces without assigned layer
            for workspace in self.workspaces.values_mut() {
                if let None = workspace.num.to_layer_id() {
                    move_workspace_to_a_new_layer!(workspace);
                }
            }
            // Fix outputs (every layer has to be on exactly one output)
            let focused_workspace_id_to_restore = self.focused_workspace_id;
            let mut layer_to_output = HashMap::new();
            for (workspace_id, workspace) in &mut self.workspaces {
                let layer_output = layer_to_output
                    .entry(workspace.num.to_layer_id().unwrap())
                    .or_insert_with(|| workspace.output.clone());
                if *layer_output != workspace.output {
                    if Some(*workspace_id) != self.focused_workspace_id {
                        swaymsg::switch_to_workspace(workspace.num).await;
                        self.focused_workspace_id = Some(*workspace_id);
                    }
                    swaymsg::move_current_workspace_to_output(&layer_output).await;
                }
            }
            if self.focused_workspace_id != focused_workspace_id_to_restore {
                swaymsg::switch_to_workspace(
                    self.workspaces
                        .get(&focused_workspace_id_to_restore.unwrap())
                        .unwrap()
                        .num,
                )
                .await;
                self.focused_workspace_id = focused_workspace_id_to_restore;
            }
        }

        async fn rename_workspace_impl(
            workspace: &mut Workspace,
            new_num: workspace::Num,
            layers: &mut BTreeMap<layer::Id, Layer>,
            next_layer_id: &mut layer::Id,
        ) {
            let layer_id = new_num.to_layer_id().unwrap();
            let old_num = mem::replace(&mut workspace.num, new_num);
            let old_name = mem::replace(&mut workspace.name, new_num.to_name());
            swaymsg::rename_workspace(&old_name, &workspace.name).await;
            // Update the target layer
            layers
                .entry(layer_id)
                .and_modify(|layer| {
                    layer.workspaces_num += 1;
                    if workspace.focused || workspace.visible {
                        layer.last_focused_workspace_num = workspace.num;
                    }
                })
                .or_insert_with(|| {
                    *next_layer_id = max(*next_layer_id, layer::Id(layer_id.0 + 1));
                    Layer {
                        workspaces_num: 1,
                        last_focused_workspace_num: workspace.num,
                    }
                });
            // Update the source layer
            if let Some(old_layer_id) = old_num.to_layer_id() {
                let workspaces_num = &mut layers.get_mut(&old_layer_id).unwrap().workspaces_num;
                *workspaces_num -= 1;
                if *workspaces_num == 0 {
                    layers.remove(&old_layer_id);
                }
            }
        }

        async fn impl_switch_to_workspace(&mut self, kind: SwitchKind, carry_focused_window: bool) {
            if let None = self.focused_workspace_id {
                return tracing::error!("Cannot switch workspace if no workspace is focused");
            }
            let old_workspace = self
                .workspaces
                .get(&self.focused_workspace_id.unwrap())
                .unwrap();
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
                    self.layers
                        .range((
                            std::ops::Bound::Excluded(old_layer_id),
                            std::ops::Bound::Unbounded,
                        ))
                        .next()
                        .or_else(|| self.layers.first_key_value())
                        .unwrap()
                        .1
                        .last_focused_workspace_num
                }
                NewLayer => {
                    let new_layer_id = self.next_layer_id;
                    self.next_layer_id.0 += 1;
                    new_layer_id.to_workspace_num(workspace::Coordinates { row: 0, col: 0 })
                }
            };
            if carry_focused_window {
                swaymsg::move_window_to_workspace(workspace_num).await;
            }
            swaymsg::switch_to_workspace(workspace_num).await;
        }

        pub(crate) async fn switch_to_workspace(&mut self, kind: SwitchKind) {
            self.impl_switch_to_workspace(kind, false).await;
        }

        pub(crate) async fn carry_to_workspace(&mut self, kind: SwitchKind) {
            self.impl_switch_to_workspace(kind, true).await;
        }
    }

    #[derive(Debug)]
    pub(crate) struct Layer {
        pub workspaces_num: u32,
        pub last_focused_workspace_num: workspace::Num,
    }

    #[derive(Debug)]
    pub(crate) struct Workspace {
        pub num: workspace::Num,
        pub name: String,
        pub focused: bool,
        pub visible: bool,
        pub output: String,
    }
}
