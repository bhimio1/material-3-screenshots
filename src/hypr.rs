//! What Hyprland knows about outputs and windows (`hyprctl -j`).

use crate::geometry::{Monitor, Rect};
use material_3_ui::sys::cmd;
use serde::Deserialize;

#[derive(Deserialize)]
struct WorkspaceRef {
    #[serde(default)]
    id: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMonitor {
    name: String,
    width: i32,
    height: i32,
    x: i32,
    y: i32,
    #[serde(default = "one")]
    scale: f32,
    #[serde(default)]
    transform: i32,
    #[serde(default)]
    focused: bool,
    #[serde(default)]
    disabled: bool,
    active_workspace: Option<WorkspaceRef>,
    special_workspace: Option<WorkspaceRef>,
}

fn one() -> f32 {
    1.0
}

/// A mapped window.
#[derive(Clone, Debug, PartialEq)]
pub struct Client {
    pub class: String,
    pub title: String,
    pub rect: Rect,
    pub workspace: i64,
    /// 0 is the focused window, 1 the one before, …
    pub focus_history: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawClient {
    #[serde(default = "yes")]
    mapped: bool,
    #[serde(default)]
    hidden: bool,
    at: [i32; 2],
    size: [i32; 2],
    workspace: Option<WorkspaceRef>,
    #[serde(default)]
    class: String,
    #[serde(default)]
    title: String,
    #[serde(rename = "focusHistoryID", default = "far")]
    focus_history_id: i64,
}

fn yes() -> bool {
    true
}

fn far() -> i64 {
    i64::MAX
}

impl RawClient {
    fn into_client(self) -> Option<Client> {
        let rect = Rect::new(self.at[0], self.at[1], self.size[0], self.size[1]);
        (self.mapped && !self.hidden && !rect.is_empty()).then(|| Client {
            class: self.class,
            title: self.title,
            rect,
            workspace: self.workspace.map(|w| w.id).unwrap_or(0),
            focus_history: self.focus_history_id,
        })
    }
}

pub fn parse_monitors(json: &str) -> Result<Vec<Monitor>, String> {
    let raw: Vec<RawMonitor> = serde_json::from_str(json).map_err(|e| format!("hyprctl monitors: {e}"))?;
    Ok(raw
        .into_iter()
        .filter(|m| !m.disabled)
        .map(|m| Monitor {
            name: m.name,
            width: m.width,
            height: m.height,
            x: m.x,
            y: m.y,
            scale: m.scale,
            transform: m.transform,
            focused: m.focused,
            active_workspace: m.active_workspace.map(|w| w.id).unwrap_or(0),
            special_workspace: m.special_workspace.map(|w| w.id).unwrap_or(0),
        })
        .collect())
}

/// Windows on the workspaces currently shown (including open special
/// workspaces), most recently focused first, so the window on top of a
/// stack wins when slurp is given overlapping boxes.
pub fn parse_clients(json: &str, monitors: &[Monitor]) -> Result<Vec<Client>, String> {
    let raw: Vec<RawClient> = serde_json::from_str(json).map_err(|e| format!("hyprctl clients: {e}"))?;
    let shown: Vec<i64> = monitors
        .iter()
        .flat_map(|m| [m.active_workspace, m.special_workspace])
        .filter(|&id| id != 0)
        .collect();
    let mut clients: Vec<Client> = raw
        .into_iter()
        .filter_map(RawClient::into_client)
        .filter(|c| shown.is_empty() || shown.contains(&c.workspace))
        .collect();
    clients.sort_by_key(|c| c.focus_history);
    Ok(clients)
}

pub fn parse_active_window(json: &str) -> Option<Client> {
    // With nothing focused hyprctl prints `{}`.
    serde_json::from_str::<RawClient>(json).ok()?.into_client()
}

fn hyprctl(what: &str) -> Result<String, String> {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() && !cmd::exists("hyprctl") {
        return Err("This needs Hyprland (hyprctl wasn't found).".into());
    }
    cmd::run("hyprctl", &["-j", what])
}

pub fn monitors() -> Result<Vec<Monitor>, String> {
    let list = parse_monitors(&hyprctl("monitors")?)?;
    if list.is_empty() {
        return Err("Hyprland reported no outputs.".into());
    }
    Ok(list)
}

pub fn clients(monitors: &[Monitor]) -> Result<Vec<Client>, String> {
    parse_clients(&hyprctl("clients")?, monitors)
}

pub fn active_window() -> Result<Client, String> {
    parse_active_window(&hyprctl("activewindow")?).ok_or_else(|| "No window is focused.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITORS: &str = r#"[
      {"id":0,"name":"eDP-1","description":"BOE","width":2880,"height":1800,"refreshRate":120.0,
       "x":0,"y":0,"activeWorkspace":{"id":1,"name":"1"},"specialWorkspace":{"id":0,"name":""},
       "reserved":[0,0,0,0],"scale":1.50,"transform":0,"focused":true,"dpmsStatus":true,"disabled":false},
      {"id":1,"name":"DP-1","width":2560,"height":1440,"x":1920,"y":0,
       "activeWorkspace":{"id":4,"name":"4"},"specialWorkspace":{"id":-98,"name":"special:magic"},
       "scale":1.0,"transform":1,"focused":false},
      {"id":2,"name":"HDMI-A-1","width":1920,"height":1080,"x":0,"y":0,"scale":1.0,"disabled":true}
    ]"#;

    const CLIENTS: &str = r#"[
      {"address":"0x1","mapped":true,"hidden":false,"at":[10,40],"size":[900,700],
       "workspace":{"id":1,"name":"1"},"floating":false,"monitor":0,"class":"kitty","title":"fish",
       "pid":10,"focusHistoryID":2},
      {"address":"0x2","mapped":true,"hidden":false,"at":[100,100],"size":[400,300],
       "workspace":{"id":1,"name":"1"},"class":"firefox","title":"Docs","focusHistoryID":0},
      {"address":"0x3","mapped":true,"hidden":false,"at":[0,0],"size":[10,10],
       "workspace":{"id":7,"name":"7"},"class":"elsewhere","title":"","focusHistoryID":1},
      {"address":"0x4","mapped":false,"hidden":false,"at":[0,0],"size":[10,10],
       "workspace":{"id":1,"name":"1"},"class":"unmapped","title":""},
      {"address":"0x5","mapped":true,"hidden":false,"at":[2000,10],"size":[500,500],
       "workspace":{"id":-98,"name":"special:magic"},"class":"scratch","title":"","focusHistoryID":5}
    ]"#;

    #[test]
    fn monitors_skip_disabled_and_keep_layout() {
        let ms = parse_monitors(MONITORS).unwrap();
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0].logical(), Rect::new(0, 0, 1920, 1200));
        assert!(ms[0].focused);
        assert_eq!(ms[1].logical(), Rect::new(1920, 0, 1440, 2560));
        assert_eq!(ms[1].special_workspace, -98);
    }

    #[test]
    fn clients_on_shown_workspaces_in_focus_order() {
        let ms = parse_monitors(MONITORS).unwrap();
        let cs = parse_clients(CLIENTS, &ms).unwrap();
        let classes: Vec<&str> = cs.iter().map(|c| c.class.as_str()).collect();
        assert_eq!(classes, ["firefox", "kitty", "scratch"]);
        assert_eq!(cs[1].rect, Rect::new(10, 40, 900, 700));
    }

    #[test]
    fn active_window() {
        let c = parse_active_window(
            r#"{"address":"0x2","mapped":true,"hidden":false,"at":[5,6],"size":[7,8],"workspace":{"id":1},"class":"foot","title":"t"}"#,
        )
        .unwrap();
        assert_eq!(c.rect, Rect::new(5, 6, 7, 8));
        assert_eq!(parse_active_window("{}"), None);
        assert!(parse_monitors("not json").is_err());
    }
}
