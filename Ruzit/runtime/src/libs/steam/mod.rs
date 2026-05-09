use std::cell::RefCell;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, OnceLock};

use mlua::{AnyUserData, Lua, MultiValue, Table, UserData, UserDataFields, UserDataMethods, Value};

use steamworks::{
    AppId, ChatMemberStateChange, Client, FriendFlags, FriendState, LobbyChatUpdate,
    LobbyDataUpdate, LobbyId, LobbyType, NotificationPosition, OverlayToStoreFlag,
    PersonaStateChange, Server, ServerMode, SingleClient, SteamError, SteamId,
    SteamServerConnectFailure, SteamServersConnected, SteamServersDisconnected,
    ValidateAuthTicketResponse,
};

use crate::libs::signal;

const DEFAULT_APP_ID: u32 = 480;

static EVENTS: OnceLock<Arc<Mutex<EventQueue>>> = OnceLock::new();
static CLIENT: OnceLock<Client> = OnceLock::new();
static SERVER: OnceLock<Server> = OnceLock::new();

thread_local! {
    static SINGLE: RefCell<Option<SingleClient>> = const { RefCell::new(None) };
    static SERVER_SINGLE: RefCell<Option<SingleClient<steamworks::ServerManager>>> = const { RefCell::new(None) };

    static LOBBY_SIGNALS: RefCell<HashMap<u64, LobbySignals>> = RefCell::new(HashMap::new());
    static SERVER_SIGNALS: RefCell<Option<ServerSignals>> = const { RefCell::new(None) };
    static GLOBAL_SIGNALS: RefCell<Option<GlobalSignals>> = const { RefCell::new(None) };

    static PENDING_LOBBY_CREATES: RefCell<Vec<Table>> = RefCell::new(Vec::new());
    static PENDING_LOBBY_JOINS: RefCell<HashMap<u64, Table>> = RefCell::new(HashMap::new());
    static PENDING_LOBBY_LISTS: RefCell<Vec<Table>> = RefCell::new(Vec::new());

    static PENDING_AVATARS: RefCell<HashMap<u64, Vec<(String, Table)>>> = RefCell::new(HashMap::new());
}

struct LobbySignals {
    member_joined: Table,
    member_left: Table,
    data_update: Table,
    disconnected: Table,
    closed: Table,
    kicked: Table,
}

struct ServerSignals {
    client_authed: Table,
    disconnected: Table,
    connected: Table,
    connect_failure: Table,
    closed: Table,
}

const RUZIT_CLOSED_KEY: &str = "_ruzit_closed";
const RUZIT_KICK_PREFIX: &str = "_ruzit_kick_";

fn lobby_kick_key(user_id: u64) -> String {
    format!("{RUZIT_KICK_PREFIX}{user_id}")
}

struct GlobalSignals {
    disconnected: Table,
    connected: Table,
    connect_failure: Table,
}

#[derive(Default)]
struct EventQueue {
    lobby_created: Vec<LobbyResult>,
    lobby_joined: Vec<LobbyResult>,
    lobby_list: Vec<LobbyListResult>,
    lobby_chat_update: Vec<LobbyChatUpdate>,
    lobby_data_update: Vec<LobbyDataUpdate>,
    server_client_authed: Vec<u64>,
    persona_changed: Vec<u64>,
    client_disconnected: Vec<String>,
    client_connected: Vec<()>,
    client_connect_failure: Vec<(String, bool)>,
    server_disconnected: Vec<String>,
    server_connected: Vec<()>,
    server_connect_failure: Vec<(String, bool)>,
}

fn steam_error_string(err: &SteamError) -> String {
    format!("{err:?}")
}

struct LobbyResult {
    success: bool,
    lobby_id: u64,
    pending_index: usize,
}

struct LobbyListResult {
    success: bool,
    lobby_ids: Vec<u64>,
    pending_index: usize,
}

fn events() -> Arc<Mutex<EventQueue>> {
    EVENTS
        .get_or_init(|| Arc::new(Mutex::new(EventQueue::default())))
        .clone()
}

pub fn is_present() -> bool {
    if !steam_lib_alongside_exe() {
        return false;
    }
    let launched_via_steam = std::env::var("SteamAppId").is_ok()
        || std::env::var("SteamGameId").is_ok()
        || std::env::var("STEAM_RUNTIME").is_ok();
    let in_test = crate::package::try_self_launcher().is_none();
    launched_via_steam || in_test
}

fn steam_lib_alongside_exe() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    let names: &[&str] = if cfg!(target_os = "windows") {
        &["steam_api64.dll", "steam_api.dll"]
    } else if cfg!(target_os = "linux") {
        &["libsteam_api.so"]
    } else if cfg!(target_os = "macos") {
        &["libsteam_api.dylib"]
    } else {
        &[]
    };
    for name in names {
        if dir.join(name).is_file() {
            return true;
        }
    }
    false
}

fn ensure_client() -> mlua::Result<&'static Client> {
    if let Some(c) = CLIENT.get() {
        return Ok(c);
    }
    if !is_present() {
        return Err(mlua::Error::RuntimeError(
            "Steam is not present (DLL missing, or not launched through Steam, or running outside a test environment). \
             Check Steam.SteamPresent before calling Steam APIs."
                .into(),
        ));
    }
    let app_id: u32 = std::env::var("RUZIT_STEAM_APPID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_APP_ID);
    let (client, single) = Client::init_app(AppId(app_id)).map_err(|e| {
        mlua::Error::RuntimeError(format!(
            "Steam init failed (app id {app_id}): {e}. Is the Steam client running?"
        ))
    })?;
    register_client_callbacks(&client);
    SINGLE.with(|c| *c.borrow_mut() = Some(single));
    let _ = CLIENT.set(client);
    if let Some(c) = CLIENT.get() {
        c.user_stats().request_current_stats();
    }
    Ok(CLIENT.get().unwrap())
}

fn register_client_callbacks(client: &Client) {
    let evq = events();

    let q = evq.clone();
    let _ = client.register_callback(move |upd: LobbyChatUpdate| {
        q.lock().unwrap().lobby_chat_update.push(upd);
    });

    let q = evq.clone();
    let _ = client.register_callback(move |upd: LobbyDataUpdate| {
        q.lock().unwrap().lobby_data_update.push(upd);
    });

    let q = evq.clone();
    let _ = client.register_callback(move |upd: PersonaStateChange| {
        q.lock().unwrap().persona_changed.push(upd.steam_id.raw());
    });

    let q = evq.clone();
    let _ = client.register_callback(move |ev: SteamServersDisconnected| {
        q.lock()
            .unwrap()
            .client_disconnected
            .push(steam_error_string(&ev.reason));
    });

    let q = evq.clone();
    let _ = client.register_callback(move |_ev: SteamServersConnected| {
        q.lock().unwrap().client_connected.push(());
    });

    let q = evq.clone();
    let _ = client.register_callback(move |ev: SteamServerConnectFailure| {
        q.lock()
            .unwrap()
            .client_connect_failure
            .push((steam_error_string(&ev.reason), ev.still_retrying));
    });
}

fn register_server_callbacks(server: &Server) {
    let evq = events();
    let q = evq.clone();
    let _ = server.register_callback(move |v: ValidateAuthTicketResponse| {
        if v.response.is_ok() {
            q.lock()
                .unwrap()
                .server_client_authed
                .push(v.steam_id.raw());
        }
    });

    let q = evq.clone();
    let _ = server.register_callback(move |ev: SteamServersDisconnected| {
        q.lock()
            .unwrap()
            .server_disconnected
            .push(steam_error_string(&ev.reason));
    });

    let q = evq.clone();
    let _ = server.register_callback(move |_ev: SteamServersConnected| {
        q.lock().unwrap().server_connected.push(());
    });

    let q = evq.clone();
    let _ = server.register_callback(move |ev: SteamServerConnectFailure| {
        q.lock()
            .unwrap()
            .server_connect_failure
            .push((steam_error_string(&ev.reason), ev.still_retrying));
    });
}

pub fn pump(lua: &Lua) {
    SINGLE.with(|c| {
        if let Some(s) = c.borrow().as_ref() {
            s.run_callbacks();
        }
    });
    SERVER_SINGLE.with(|c| {
        if let Some(s) = c.borrow().as_ref() {
            s.run_callbacks();
        }
    });
    let _ = drain_events(lua);
}

fn drain_events(lua: &Lua) -> mlua::Result<()> {
    let drained = {
        let arc = events();
        let mut q = arc.lock().unwrap();
        std::mem::take(&mut *q)
    };

    for result in drained.lobby_created {
        let table_opt = PENDING_LOBBY_CREATES.with(|p| {
            let mut p = p.borrow_mut();
            if result.pending_index < p.len() {
                Some(p.remove(result.pending_index))
            } else {
                p.pop()
            }
        });
        if let Some(sig) = table_opt {
            let mut args = MultiValue::new();
            if result.success {
                let lobby = make_lobby_userdata(lua, result.lobby_id)?;
                args.push_back(Value::UserData(lobby));
            } else {
                args.push_back(Value::Nil);
                args.push_back(Value::String(lua.create_string("CreateLobby failed")?));
            }
            signal::fire(lua, &sig, args)?;
        }
    }

    for result in drained.lobby_joined {
        let table_opt = PENDING_LOBBY_JOINS.with(|p| p.borrow_mut().remove(&result.lobby_id));
        if let Some(sig) = table_opt {
            let mut args = MultiValue::new();
            if result.success {
                let lobby = make_lobby_userdata(lua, result.lobby_id)?;
                args.push_back(Value::UserData(lobby));
            } else {
                args.push_back(Value::Nil);
                args.push_back(Value::String(lua.create_string("JoinLobby failed")?));
            }
            signal::fire(lua, &sig, args)?;
        }
    }

    for result in drained.lobby_list {
        let table_opt = PENDING_LOBBY_LISTS.with(|p| {
            let mut p = p.borrow_mut();
            if result.pending_index < p.len() {
                Some(p.remove(result.pending_index))
            } else {
                p.pop()
            }
        });
        if let Some(sig) = table_opt {
            let arr = lua.create_table()?;
            if result.success {
                for (i, id) in result.lobby_ids.into_iter().enumerate() {
                    let info = lua.create_table()?;
                    info.set("ID", id.to_string())?;
                    if let Some(client) = CLIENT.get() {
                        let mm = client.matchmaking();
                        let lid = LobbyId::from_raw(id);
                        info.set("MemberCount", mm.lobby_member_count(lid) as i64)?;
                        if let Some(name) = mm.lobby_data(lid, "name") {
                            info.set("Name", name.to_string())?;
                        }
                    }
                    arr.set(i + 1, info)?;
                }
            }
            let mut args = MultiValue::new();
            args.push_back(Value::Table(arr));
            signal::fire(lua, &sig, args)?;
        }
    }

    let local_steam_id = CLIENT.get().map(|c| c.user().steam_id().raw()).unwrap_or(0);

    for upd in drained.lobby_chat_update {
        let lobby_raw = upd.lobby.raw();
        let user_raw = upd.user_changed.raw();
        let joined = matches!(upd.member_state_change, ChatMemberStateChange::Entered);
        let sig_opt = LOBBY_SIGNALS.with(|m| {
            m.borrow().get(&lobby_raw).map(|s| {
                if joined {
                    s.member_joined.clone()
                } else {
                    s.member_left.clone()
                }
            })
        });
        if let Some(sig) = sig_opt {
            let mut args = MultiValue::new();
            args.push_back(Value::String(lua.create_string(&user_raw.to_string())?));
            signal::fire(lua, &sig, args)?;
        }

        let local_disconnect_reason: Option<&'static str> =
            if user_raw == local_steam_id && local_steam_id != 0 {
                match upd.member_state_change {
                    ChatMemberStateChange::Disconnected => Some("Disconnected"),
                    ChatMemberStateChange::Kicked => Some("Kicked"),
                    ChatMemberStateChange::Banned => Some("Banned"),
                    _ => None,
                }
            } else {
                None
            };

        if let Some(reason) = local_disconnect_reason {
            let dc_sig = LOBBY_SIGNALS
                .with(|m| m.borrow().get(&lobby_raw).map(|s| s.disconnected.clone()));
            if let Some(sig) = dc_sig {
                let mut args = MultiValue::new();
                args.push_back(Value::String(lua.create_string(reason)?));
                signal::fire(lua, &sig, args)?;
            }
            LOBBY_SIGNALS.with(|m| {
                m.borrow_mut().remove(&lobby_raw);
            });
        }
    }

    for upd in drained.lobby_data_update {
        let lobby_raw = upd.lobby.raw();
        let member_raw = upd.member.raw();
        let sig_opt =
            LOBBY_SIGNALS.with(|m| m.borrow().get(&lobby_raw).map(|s| s.data_update.clone()));
        if let Some(sig) = sig_opt {
            let mut args = MultiValue::new();
            if member_raw == lobby_raw {
                args.push_back(Value::Nil);
            } else {
                args.push_back(Value::String(lua.create_string(&member_raw.to_string())?));
            }
            signal::fire(lua, &sig, args)?;
        }

        if member_raw == lobby_raw {
            if let Some(c) = CLIENT.get() {
                let lobby_id = LobbyId::from_raw(lobby_raw);
                let my_id = c.user().steam_id().raw();
                let mm = c.matchmaking();

                let i_was_kicked = my_id != 0
                    && mm.lobby_data(lobby_id, &lobby_kick_key(my_id))
                        .map(|v| v == "1")
                        .unwrap_or(false);

                if i_was_kicked {
                    let kick_sig = LOBBY_SIGNALS
                        .with(|m| m.borrow().get(&lobby_raw).map(|s| s.kicked.clone()));
                    if let Some(sig) = kick_sig {
                        let mut args = MultiValue::new();
                        args.push_back(Value::String(
                            lua.create_string(&mm.lobby_owner(lobby_id).raw().to_string())?,
                        ));
                        signal::fire(lua, &sig, args)?;
                    }
                    mm.leave_lobby(lobby_id);
                    LOBBY_SIGNALS.with(|m| m.borrow_mut().remove(&lobby_raw));
                    continue;
                }

                let lobby_closed = mm
                    .lobby_data(lobby_id, RUZIT_CLOSED_KEY)
                    .map(|v| v == "1")
                    .unwrap_or(false);
                let i_am_owner = mm.lobby_owner(lobby_id).raw() == my_id;

                if lobby_closed && !i_am_owner {
                    let close_sig = LOBBY_SIGNALS
                        .with(|m| m.borrow().get(&lobby_raw).map(|s| s.closed.clone()));
                    if let Some(sig) = close_sig {
                        signal::fire(lua, &sig, MultiValue::new())?;
                    }
                    mm.leave_lobby(lobby_id);
                    LOBBY_SIGNALS.with(|m| m.borrow_mut().remove(&lobby_raw));
                }
            }
        }
    }

    for id in drained.server_client_authed {
        let sig_opt = SERVER_SIGNALS.with(|s| s.borrow().as_ref().map(|s| s.client_authed.clone()));
        if let Some(sig) = sig_opt {
            let mut args = MultiValue::new();
            args.push_back(Value::String(lua.create_string(&id.to_string())?));
            signal::fire(lua, &sig, args)?;
        }
    }

    for id in drained.persona_changed {
        let pending = PENDING_AVATARS.with(|m| m.borrow_mut().remove(&id));
        if let Some(entries) = pending {
            if let Some(c) = CLIENT.get() {
                let friend = c.friends().get_friend(SteamId::from_raw(id));
                for (size, signal_table) in entries {
                    let mut args = MultiValue::new();
                    let result = avatar_to_image(lua, &friend, &size)?;
                    args.push_back(result);
                    signal::fire(lua, &signal_table, args)?;
                }
            }
        }
    }

    for reason in drained.client_disconnected {
        let sig_opt =
            GLOBAL_SIGNALS.with(|s| s.borrow().as_ref().map(|s| s.disconnected.clone()));
        if let Some(sig) = sig_opt {
            let mut args = MultiValue::new();
            args.push_back(Value::String(lua.create_string(&reason)?));
            signal::fire(lua, &sig, args)?;
        }
    }

    for _ in drained.client_connected {
        let sig_opt = GLOBAL_SIGNALS.with(|s| s.borrow().as_ref().map(|s| s.connected.clone()));
        if let Some(sig) = sig_opt {
            signal::fire(lua, &sig, MultiValue::new())?;
        }
    }

    for (reason, retry) in drained.client_connect_failure {
        let sig_opt =
            GLOBAL_SIGNALS.with(|s| s.borrow().as_ref().map(|s| s.connect_failure.clone()));
        if let Some(sig) = sig_opt {
            let mut args = MultiValue::new();
            args.push_back(Value::String(lua.create_string(&reason)?));
            args.push_back(Value::Boolean(retry));
            signal::fire(lua, &sig, args)?;
        }
    }

    for reason in drained.server_disconnected {
        let sig_opt =
            SERVER_SIGNALS.with(|s| s.borrow().as_ref().map(|s| s.disconnected.clone()));
        if let Some(sig) = sig_opt {
            let mut args = MultiValue::new();
            args.push_back(Value::String(lua.create_string(&reason)?));
            signal::fire(lua, &sig, args)?;
        }
    }

    for _ in drained.server_connected {
        let sig_opt =
            SERVER_SIGNALS.with(|s| s.borrow().as_ref().map(|s| s.connected.clone()));
        if let Some(sig) = sig_opt {
            signal::fire(lua, &sig, MultiValue::new())?;
        }
    }

    for (reason, retry) in drained.server_connect_failure {
        let sig_opt =
            SERVER_SIGNALS.with(|s| s.borrow().as_ref().map(|s| s.connect_failure.clone()));
        if let Some(sig) = sig_opt {
            let mut args = MultiValue::new();
            args.push_back(Value::String(lua.create_string(&reason)?));
            args.push_back(Value::Boolean(retry));
            signal::fire(lua, &sig, args)?;
        }
    }

    Ok(())
}

fn make_lobby_userdata(lua: &Lua, raw: u64) -> mlua::Result<AnyUserData> {
    let member_joined = signal::new_instance(lua)?;
    let member_left = signal::new_instance(lua)?;
    let data_update = signal::new_instance(lua)?;
    let disconnected = signal::new_instance(lua)?;
    let closed = signal::new_instance(lua)?;
    let kicked = signal::new_instance(lua)?;
    LOBBY_SIGNALS.with(|m| {
        m.borrow_mut().insert(
            raw,
            LobbySignals {
                member_joined: member_joined.clone(),
                member_left: member_left.clone(),
                data_update: data_update.clone(),
                disconnected: disconnected.clone(),
                closed: closed.clone(),
                kicked: kicked.clone(),
            },
        );
    });
    let lobby = LobbyHandle {
        id: raw,
        member_joined,
        member_left,
        data_update,
        disconnected,
        closed,
        kicked,
    };
    lua.create_userdata(lobby)
}

pub struct LobbyHandle {
    id: u64,
    member_joined: Table,
    member_left: Table,
    data_update: Table,
    disconnected: Table,
    closed: Table,
    kicked: Table,
}

impl UserData for LobbyHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("ID", |_, this| Ok(this.id.to_string()));
        f.add_field_method_get("Owner", |_, this| {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.matchmaking()
                .lobby_owner(LobbyId::from_raw(this.id))
                .raw()
                .to_string())
        });
        f.add_field_method_get("OnMemberJoined", |_, this| Ok(this.member_joined.clone()));
        f.add_field_method_get("OnMemberLeft", |_, this| Ok(this.member_left.clone()));
        f.add_field_method_get("OnDataUpdate", |_, this| Ok(this.data_update.clone()));
        f.add_field_method_get("OnDisconnected", |_, this| Ok(this.disconnected.clone()));
        f.add_field_method_get("OnClose", |_, this| Ok(this.closed.clone()));
        f.add_field_method_get("OnKick", |_, this| Ok(this.kicked.clone()));
        f.add_field_method_get("IsOwner", |_, this| {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.matchmaking().lobby_owner(LobbyId::from_raw(this.id)) == c.user().steam_id())
        });
        f.add_field_method_get("IsClosed", |_, this| {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.matchmaking()
                .lobby_data(LobbyId::from_raw(this.id), RUZIT_CLOSED_KEY)
                .map(|v| v == "1")
                .unwrap_or(false))
        });
    }
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Members", |lua, this, _: ()| -> mlua::Result<Table> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let arr = lua.create_table()?;
            for (i, id) in c
                .matchmaking()
                .lobby_members(LobbyId::from_raw(this.id))
                .into_iter()
                .enumerate()
            {
                arr.set(i + 1, id.raw().to_string())?;
            }
            Ok(arr)
        });
        m.add_method(
            "SetData",
            |_, this, (key, value): (String, String)| -> mlua::Result<()> {
                if key.starts_with("_ruzit_") {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Lobby:SetData: keys starting with '_ruzit_' are reserved (got {key:?})"
                    )));
                }
                let c = CLIENT
                    .get()
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
                c.matchmaking()
                    .set_lobby_data(LobbyId::from_raw(this.id), &key, &value);
                Ok(())
            },
        );
        m.add_method(
            "GetData",
            |_, this, key: String| -> mlua::Result<Option<String>> {
                let c = CLIENT
                    .get()
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
                Ok(c.matchmaking()
                    .lobby_data(LobbyId::from_raw(this.id), &key)
                    .map(|s| s.to_string()))
            },
        );
        m.add_method("SendChat", |_, this, msg: String| -> mlua::Result<()> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            c.matchmaking()
                .send_lobby_chat_message(LobbyId::from_raw(this.id), msg.as_bytes())
                .map_err(|e| mlua::Error::RuntimeError(format!("SendChat: {e:?}")))?;
            Ok(())
        });
        m.add_method("Leave", |_, this, _: ()| -> mlua::Result<()> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            c.matchmaking().leave_lobby(LobbyId::from_raw(this.id));
            LOBBY_SIGNALS.with(|m| m.borrow_mut().remove(&this.id));
            Ok(())
        });
        m.add_method(
            "SetJoinable",
            |_, this, joinable: bool| -> mlua::Result<bool> {
                let c = CLIENT
                    .get()
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
                Ok(c.matchmaking()
                    .set_lobby_joinable(LobbyId::from_raw(this.id), joinable))
            },
        );
        m.add_method("Close", |lua, this, _: ()| -> mlua::Result<()> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let lobby_id = LobbyId::from_raw(this.id);
            if c.matchmaking().lobby_owner(lobby_id) != c.user().steam_id() {
                return Err(mlua::Error::RuntimeError(
                    "Lobby:Close: only the lobby owner can close the lobby".into(),
                ));
            }
            c.matchmaking().set_lobby_joinable(lobby_id, false);
            c.matchmaking()
                .set_lobby_data(lobby_id, RUZIT_CLOSED_KEY, "1");
            let close_sig = LOBBY_SIGNALS
                .with(|m| m.borrow().get(&this.id).map(|s| s.closed.clone()));
            if let Some(sig) = close_sig {
                signal::fire(lua, &sig, MultiValue::new())?;
            }
            c.matchmaking().leave_lobby(lobby_id);
            LOBBY_SIGNALS.with(|m| m.borrow_mut().remove(&this.id));
            Ok(())
        });
        m.add_method(
            "DeleteData",
            |_, this, key: String| -> mlua::Result<bool> {
                if key.starts_with("_ruzit_") {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Lobby:DeleteData: keys starting with '_ruzit_' are reserved (got {key:?})"
                    )));
                }
                let c = CLIENT
                    .get()
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
                Ok(c.matchmaking()
                    .delete_lobby_data(LobbyId::from_raw(this.id), &key))
            },
        );
        m.add_method("GetAllData", |lua, this, _: ()| -> mlua::Result<Table> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let lobby_id = LobbyId::from_raw(this.id);
            let mm = c.matchmaking();
            let count = mm.lobby_data_count(lobby_id);
            let t = lua.create_table()?;
            for i in 0..count {
                if let Some((k, v)) = mm.lobby_data_by_index(lobby_id, i) {
                    if k.starts_with("_ruzit_") {
                        continue;
                    }
                    t.set(k, v)?;
                }
            }
            Ok(t)
        });
        m.add_method("MemberLimit", |_, this, _: ()| -> mlua::Result<Option<i64>> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.matchmaking()
                .lobby_member_limit(LobbyId::from_raw(this.id))
                .map(|v| v as i64))
        });
        m.add_method("ShowInviteDialog", |_, this, _: ()| -> mlua::Result<()> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            c.friends()
                .activate_invite_dialog(LobbyId::from_raw(this.id));
            Ok(())
        });
        m.add_method("Kick", |_, this, user_id_str: String| -> mlua::Result<()> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let lobby_id = LobbyId::from_raw(this.id);
            if c.matchmaking().lobby_owner(lobby_id) != c.user().steam_id() {
                return Err(mlua::Error::RuntimeError(
                    "Lobby:Kick: only the lobby owner can kick".into(),
                ));
            }
            let user_raw: u64 = user_id_str
                .parse()
                .map_err(|e| mlua::Error::RuntimeError(format!("Lobby:Kick: bad id: {e}")))?;
            if user_raw == c.user().steam_id().raw() {
                return Err(mlua::Error::RuntimeError(
                    "Lobby:Kick: can't kick yourself; call :Close() or :Leave() instead".into(),
                ));
            }
            c.matchmaking()
                .set_lobby_data(lobby_id, &lobby_kick_key(user_raw), "1");
            Ok(())
        });
    }
}

pub struct ServerHandle;

impl UserData for ServerHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("ID", |_, _| {
            let s = SERVER
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam server not started".into()))?;
            Ok(s.steam_id().raw().to_string())
        });
        f.add_field_method_get("OnClientAuthed", |_, _| {
            SERVER_SIGNALS.with(|s| {
                s.borrow()
                    .as_ref()
                    .map(|sig| sig.client_authed.clone())
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam server not started".into()))
            })
        });
        f.add_field_method_get("OnDisconnected", |_, _| {
            SERVER_SIGNALS.with(|s| {
                s.borrow()
                    .as_ref()
                    .map(|sig| sig.disconnected.clone())
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam server not started".into()))
            })
        });
        f.add_field_method_get("OnConnected", |_, _| {
            SERVER_SIGNALS.with(|s| {
                s.borrow()
                    .as_ref()
                    .map(|sig| sig.connected.clone())
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam server not started".into()))
            })
        });
        f.add_field_method_get("OnConnectFailure", |_, _| {
            SERVER_SIGNALS.with(|s| {
                s.borrow()
                    .as_ref()
                    .map(|sig| sig.connect_failure.clone())
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam server not started".into()))
            })
        });
        f.add_field_method_get("OnClose", |_, _| {
            SERVER_SIGNALS.with(|s| {
                s.borrow()
                    .as_ref()
                    .map(|sig| sig.closed.clone())
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam server not started".into()))
            })
        });
    }
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("SetServerName", |_, _, name: String| -> mlua::Result<()> {
            let s = SERVER
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam server not started".into()))?;
            s.set_server_name(&name);
            Ok(())
        });
        m.add_method("SetMapName", |_, _, name: String| -> mlua::Result<()> {
            let s = SERVER
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam server not started".into()))?;
            s.set_map_name(&name);
            Ok(())
        });
        m.add_method("SetMaxPlayers", |_, _, max: i32| -> mlua::Result<()> {
            let s = SERVER
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam server not started".into()))?;
            s.set_max_players(max);
            Ok(())
        });
        m.add_method("LogOnAnonymous", |_, _, _: ()| -> mlua::Result<()> {
            let s = SERVER
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam server not started".into()))?;
            s.log_on_anonymous();
            Ok(())
        });
        m.add_method("Stop", |lua, _, _: ()| -> mlua::Result<()> {
            let close_sig = SERVER_SIGNALS
                .with(|s| s.borrow().as_ref().map(|sig| sig.closed.clone()));
            if let Some(sig) = close_sig {
                signal::fire(lua, &sig, MultiValue::new())?;
            }
            SERVER_SINGLE.with(|c| *c.borrow_mut() = None);
            SERVER_SIGNALS.with(|s| *s.borrow_mut() = None);
            Ok(())
        });
        m.add_method("Close", |lua, _, _: ()| -> mlua::Result<()> {
            let close_sig = SERVER_SIGNALS
                .with(|s| s.borrow().as_ref().map(|sig| sig.closed.clone()));
            if let Some(sig) = close_sig {
                signal::fire(lua, &sig, MultiValue::new())?;
            }
            SERVER_SINGLE.with(|c| *c.borrow_mut() = None);
            SERVER_SIGNALS.with(|s| *s.borrow_mut() = None);
            Ok(())
        });
        m.add_method("Kick", |_, _, user_id_str: String| -> mlua::Result<()> {
            let s = SERVER
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam server not started".into()))?;
            let raw: u64 = user_id_str
                .parse()
                .map_err(|e| mlua::Error::RuntimeError(format!("Server:Kick: bad id: {e}")))?;
            s.end_authentication_session(SteamId::from_raw(raw));
            Ok(())
        });
    }
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let _ = ensure_client();

    let t = lua.create_table()?;

    let on_disconnected = signal::new_instance(lua)?;
    let on_connected = signal::new_instance(lua)?;
    let on_connect_failure = signal::new_instance(lua)?;
    GLOBAL_SIGNALS.with(|s| {
        *s.borrow_mut() = Some(GlobalSignals {
            disconnected: on_disconnected.clone(),
            connected: on_connected.clone(),
            connect_failure: on_connect_failure.clone(),
        });
    });
    t.set("OnDisconnected", on_disconnected)?;
    t.set("OnConnected", on_connected)?;
    t.set("OnConnectFailure", on_connect_failure)?;

    t.set("SteamPresent", is_present())?;
    t.set("User", build_user(lua)?)?;
    t.set("Achievements", build_achievements(lua)?)?;
    t.set("Stats", build_stats(lua)?)?;
    t.set("Lobby", build_lobby(lua)?)?;
    t.set("Server", build_server(lua)?)?;
    t.set("Overlay", build_overlay(lua)?)?;
    t.set("App", build_app(lua)?)?;
    t.set("Utils", build_utils(lua)?)?;
    t.set("RichPresence", build_rich_presence(lua)?)?;
    t.set("Cloud", build_cloud(lua)?)?;
    t.set("Workshop", build_workshop(lua)?)?;
    t.set("RemotePlay", build_remote_play(lua)?)?;
    t.set("Launch", build_launch(lua)?)?;
    Ok(t)
}

fn build_launch(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    t.set(
        "GetCommandLine",
        lua.create_function(|_, _: ()| -> mlua::Result<String> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.apps().launch_command_line())
        })?,
    )?;

    t.set(
        "GetQueryParam",
        lua.create_function(|_, key: String| -> mlua::Result<Option<String>> {
            let line = match CLIENT.get() {
                Some(c) => c.apps().launch_command_line(),
                None => return Ok(None),
            };
            Ok(parse_query_param(&line, &key))
        })?,
    )?;

    t.set(
        "GetCurrentBeta",
        lua.create_function(|_, _: ()| -> mlua::Result<Option<String>> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.apps().current_beta_name())
        })?,
    )?;

    t.set(
        "GetRunType",
        lua.create_function(|lua, _: ()| -> mlua::Result<Table> {
            let info = lua.create_table()?;
            let client = CLIENT.get();
            let app_id_env = std::env::var("RUZIT_STEAM_APPID")
                .ok()
                .and_then(|s| s.parse::<u32>().ok());

            let steam_command = client
                .map(|c| c.apps().launch_command_line())
                .unwrap_or_default();

            let steam_env_signals = [
                "SteamAppId",
                "SteamGameId",
                "SteamClientLaunch",
                "STEAM_RUNTIME",
            ];
            let from_steam = steam_env_signals.iter().any(|k| std::env::var(k).is_ok())
                || !steam_command.is_empty();

            let source = if !steam_command.is_empty() {
                "steam-url"
            } else if from_steam {
                "steam"
            } else {
                "direct"
            };

            info.set("Source", source)?;
            info.set("WasLaunchedFromSteam", from_steam)?;
            info.set("CommandLine", steam_command)?;

            let app_id = client
                .map(|c| c.utils().app_id().0)
                .or(app_id_env)
                .unwrap_or(0);
            info.set("AppId", app_id)?;

            if let Some(c) = client {
                info.set("BuildId", c.apps().app_build_id())?;
                info.set("OwnerID", c.apps().app_owner().raw().to_string())?;
                if let Some(beta) = c.apps().current_beta_name() {
                    info.set("BetaBranch", beta)?;
                }
                info.set("InstallDir", c.apps().app_install_dir(AppId(app_id)))?;
                info.set("Language", c.apps().current_game_language())?;
            }

            let args = lua.create_table()?;
            for (i, a) in std::env::args().skip(1).enumerate() {
                args.set(i + 1, a)?;
            }
            info.set("ProcessArgs", args)?;

            let env = lua.create_table()?;
            for key in [
                "SteamAppId",
                "SteamGameId",
                "SteamUser",
                "STEAM_RUNTIME",
                "STEAM_COMPAT_DATA_PATH",
                "RUZIT_STEAM_APPID",
            ] {
                if let Ok(v) = std::env::var(key) {
                    env.set(key, v)?;
                }
            }
            info.set("Env", env)?;

            Ok(info)
        })?,
    )?;

    Ok(t)
}

fn parse_query_param(line: &str, key: &str) -> Option<String> {
    let needle = format!("+{key}");
    let mut iter = line.split_whitespace();
    while let Some(tok) = iter.next() {
        if tok.eq_ignore_ascii_case(&needle) {
            return iter.next().map(|s| s.to_string());
        }
    }
    None
}

fn build_app(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "IsAppInstalled",
        lua.create_function(|_, app_id: u32| -> mlua::Result<bool> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.apps().is_app_installed(AppId(app_id)))
        })?,
    )?;
    t.set(
        "IsDlcInstalled",
        lua.create_function(|_, app_id: u32| -> mlua::Result<bool> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.apps().is_dlc_installed(AppId(app_id)))
        })?,
    )?;
    t.set(
        "IsSubscribed",
        lua.create_function(|_, _: ()| -> mlua::Result<bool> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.apps().is_subscribed())
        })?,
    )?;
    t.set(
        "IsVacBanned",
        lua.create_function(|_, _: ()| -> mlua::Result<bool> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.apps().is_vac_banned())
        })?,
    )?;
    t.set(
        "BuildId",
        lua.create_function(|_, _: ()| -> mlua::Result<i32> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.apps().app_build_id())
        })?,
    )?;
    t.set(
        "InstallDir",
        lua.create_function(|_, app_id: u32| -> mlua::Result<String> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.apps().app_install_dir(AppId(app_id)))
        })?,
    )?;
    t.set(
        "OwnerID",
        lua.create_function(|_, _: ()| -> mlua::Result<String> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.apps().app_owner().raw().to_string())
        })?,
    )?;
    t.set(
        "CurrentLanguage",
        lua.create_function(|_, _: ()| -> mlua::Result<String> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.apps().current_game_language())
        })?,
    )?;
    t.set(
        "AvailableLanguages",
        lua.create_function(|lua, _: ()| -> mlua::Result<Table> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let arr = lua.create_table()?;
            for (i, lang) in c.apps().available_game_languages().into_iter().enumerate() {
                arr.set(i + 1, lang)?;
            }
            Ok(arr)
        })?,
    )?;
    t.set(
        "BetaName",
        lua.create_function(|_, _: ()| -> mlua::Result<Option<String>> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.apps().current_beta_name())
        })?,
    )?;
    t.set(
        "LaunchCommandLine",
        lua.create_function(|_, _: ()| -> mlua::Result<String> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.apps().launch_command_line())
        })?,
    )?;
    Ok(t)
}

fn build_utils(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "UILanguage",
        lua.create_function(|_, _: ()| -> mlua::Result<String> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.utils().ui_language())
        })?,
    )?;
    t.set(
        "IpCountry",
        lua.create_function(|_, _: ()| -> mlua::Result<String> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.utils().ip_country())
        })?,
    )?;
    t.set(
        "ServerTime",
        lua.create_function(|_, _: ()| -> mlua::Result<i64> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.utils().get_server_real_time() as i64)
        })?,
    )?;
    t.set(
        "IsSteamDeck",
        lua.create_function(|_, _: ()| -> mlua::Result<bool> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.utils().is_steam_running_on_steam_deck())
        })?,
    )?;
    t.set(
        "AppID",
        lua.create_function(|_, _: ()| -> mlua::Result<u32> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.utils().app_id().0)
        })?,
    )?;
    t.set(
        "SetOverlayPosition",
        lua.create_function(|_, position: String| -> mlua::Result<()> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let pos = match position.as_str() {
                "TopLeft" => NotificationPosition::TopLeft,
                "BottomLeft" => NotificationPosition::BottomLeft,
                "BottomRight" => NotificationPosition::BottomRight,
                _ => NotificationPosition::TopRight,
            };
            c.utils().set_overlay_notification_position(pos);
            Ok(())
        })?,
    )?;
    Ok(t)
}

fn build_rich_presence(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "Set",
        lua.create_function(
            |_, (key, value): (String, Option<String>)| -> mlua::Result<bool> {
                let c = CLIENT
                    .get()
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
                Ok(c.friends().set_rich_presence(&key, value.as_deref()))
            },
        )?,
    )?;
    t.set(
        "Clear",
        lua.create_function(|_, _: ()| -> mlua::Result<()> {
            if let Some(c) = CLIENT.get() {
                c.friends().clear_rich_presence();
            }
            Ok(())
        })?,
    )?;
    Ok(t)
}

fn build_cloud(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "IsEnabledForAccount",
        lua.create_function(|_, _: ()| -> mlua::Result<bool> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.remote_storage().is_cloud_enabled_for_account())
        })?,
    )?;
    t.set(
        "IsEnabledForApp",
        lua.create_function(|_, _: ()| -> mlua::Result<bool> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.remote_storage().is_cloud_enabled_for_app())
        })?,
    )?;
    t.set(
        "SetEnabledForApp",
        lua.create_function(|_, enabled: bool| -> mlua::Result<()> {
            if let Some(c) = CLIENT.get() {
                c.remote_storage().set_cloud_enabled_for_app(enabled);
            }
            Ok(())
        })?,
    )?;
    t.set(
        "Files",
        lua.create_function(|lua, _: ()| -> mlua::Result<Table> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let arr = lua.create_table()?;
            for (i, f) in c.remote_storage().files().into_iter().enumerate() {
                let info = lua.create_table()?;
                info.set("Name", f.name.clone())?;
                info.set("Size", f.size as i64)?;
                arr.set(i + 1, info)?;
            }
            Ok(arr)
        })?,
    )?;
    t.set(
        "Exists",
        lua.create_function(|_, name: String| -> mlua::Result<bool> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.remote_storage().file(&name).exists())
        })?,
    )?;
    t.set(
        "Read",
        lua.create_function(|lua, name: String| -> mlua::Result<Value> {
            use std::io::Read;
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let file = c.remote_storage().file(&name);
            if !file.exists() {
                return Ok(Value::Nil);
            }
            let mut reader = file.read();
            let mut buf = Vec::new();
            reader
                .read_to_end(&mut buf)
                .map_err(|e| mlua::Error::RuntimeError(format!("Cloud.Read: {e}")))?;
            Ok(Value::String(lua.create_string(&buf)?))
        })?,
    )?;
    t.set(
        "Write",
        lua.create_function(
            |_, (name, data): (String, mlua::String)| -> mlua::Result<()> {
                use std::io::Write;
                let c = CLIENT
                    .get()
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
                let file = c.remote_storage().file(&name);
                let mut writer = file.write();
                writer
                    .write_all(&data.as_bytes())
                    .map_err(|e| mlua::Error::RuntimeError(format!("Cloud.Write: {e}")))?;
                Ok(())
            },
        )?,
    )?;
    t.set(
        "Delete",
        lua.create_function(|_, name: String| -> mlua::Result<bool> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.remote_storage().file(&name).delete())
        })?,
    )?;
    Ok(t)
}

fn build_workshop(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "SubscribedItems",
        lua.create_function(|lua, _: ()| -> mlua::Result<Table> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let arr = lua.create_table()?;
            for (i, item) in c.ugc().subscribed_items().into_iter().enumerate() {
                arr.set(i + 1, item.0.to_string())?;
            }
            Ok(arr)
        })?,
    )?;
    t.set(
        "ItemState",
        lua.create_function(|lua, id_str: String| -> mlua::Result<Table> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let raw: u64 = id_str
                .parse()
                .map_err(|e| mlua::Error::RuntimeError(format!("ItemState: bad id: {e}")))?;
            let state = c.ugc().item_state(steamworks::PublishedFileId(raw));
            let info = lua.create_table()?;
            info.set(
                "Subscribed",
                state.contains(steamworks::ItemState::SUBSCRIBED),
            )?;
            info.set(
                "Installed",
                state.contains(steamworks::ItemState::INSTALLED),
            )?;
            info.set(
                "NeedsUpdate",
                state.contains(steamworks::ItemState::NEEDS_UPDATE),
            )?;
            info.set(
                "Downloading",
                state.contains(steamworks::ItemState::DOWNLOADING),
            )?;
            info.set(
                "DownloadPending",
                state.contains(steamworks::ItemState::DOWNLOAD_PENDING),
            )?;
            Ok(info)
        })?,
    )?;
    t.set(
        "InstallInfo",
        lua.create_function(|lua, id_str: String| -> mlua::Result<Value> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let raw: u64 = id_str
                .parse()
                .map_err(|e| mlua::Error::RuntimeError(format!("InstallInfo: bad id: {e}")))?;
            match c.ugc().item_install_info(steamworks::PublishedFileId(raw)) {
                Some(info) => {
                    let t = lua.create_table()?;
                    t.set("Folder", info.folder)?;
                    t.set("SizeOnDisk", info.size_on_disk as i64)?;
                    t.set("Timestamp", info.timestamp as i64)?;
                    Ok(Value::Table(t))
                }
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    t.set(
        "DownloadProgress",
        lua.create_function(|lua, id_str: String| -> mlua::Result<Value> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let raw: u64 = id_str
                .parse()
                .map_err(|e| mlua::Error::RuntimeError(format!("DownloadProgress: bad id: {e}")))?;
            match c.ugc().item_download_info(steamworks::PublishedFileId(raw)) {
                Some((bytes_downloaded, bytes_total)) => {
                    let t = lua.create_table()?;
                    t.set("Downloaded", bytes_downloaded as i64)?;
                    t.set("Total", bytes_total as i64)?;
                    Ok(Value::Table(t))
                }
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    t.set(
        "DownloadItem",
        lua.create_function(
            |_, (id_str, high_priority): (String, Option<bool>)| -> mlua::Result<bool> {
                let c = CLIENT
                    .get()
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
                let raw: u64 = id_str
                    .parse()
                    .map_err(|e| mlua::Error::RuntimeError(format!("DownloadItem: bad id: {e}")))?;
                Ok(c.ugc().download_item(
                    steamworks::PublishedFileId(raw),
                    high_priority.unwrap_or(false),
                ))
            },
        )?,
    )?;
    t.set(
        "GetItem",
        lua.create_function(|lua, id_str: String| -> mlua::Result<Value> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let raw: u64 = id_str
                .parse()
                .map_err(|e| mlua::Error::RuntimeError(format!("GetItem: bad id: {e}")))?;
            let pid = steamworks::PublishedFileId(raw);
            let state = c.ugc().item_state(pid);
            let info = lua.create_table()?;
            info.set("ID", id_str)?;
            info.set(
                "Subscribed",
                state.contains(steamworks::ItemState::SUBSCRIBED),
            )?;
            info.set(
                "Installed",
                state.contains(steamworks::ItemState::INSTALLED),
            )?;
            info.set(
                "NeedsUpdate",
                state.contains(steamworks::ItemState::NEEDS_UPDATE),
            )?;
            info.set(
                "Downloading",
                state.contains(steamworks::ItemState::DOWNLOADING),
            )?;
            info.set(
                "DownloadPending",
                state.contains(steamworks::ItemState::DOWNLOAD_PENDING),
            )?;
            if let Some(install) = c.ugc().item_install_info(pid) {
                info.set("Folder", install.folder.clone())?;
                info.set("SizeOnDisk", install.size_on_disk as i64)?;
                info.set("Timestamp", install.timestamp as i64)?;
                let folder = install.folder.clone();
                info.set(
                    "Files",
                    lua.create_function(move |lua, _: ()| -> mlua::Result<Table> {
                        let arr = lua.create_table()?;
                        let mut i = 0i64;
                        if let Ok(rd) = std::fs::read_dir(&folder) {
                            for entry in rd.flatten() {
                                if let Ok(ft) = entry.file_type() {
                                    if ft.is_file() {
                                        i += 1;
                                        arr.set(i, entry.path().to_string_lossy().into_owned())?;
                                    }
                                }
                            }
                        }
                        Ok(arr)
                    })?,
                )?;
            }
            if let Some((dl, total)) = c.ugc().item_download_info(pid) {
                let dlt = lua.create_table()?;
                dlt.set("Downloaded", dl as i64)?;
                dlt.set("Total", total as i64)?;
                info.set("DownloadProgress", dlt)?;
            }
            Ok(Value::Table(info))
        })?,
    )?;
    Ok(t)
}

fn build_remote_play(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "Sessions",
        lua.create_function(|lua, _: ()| -> mlua::Result<Table> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let arr = lua.create_table()?;
            for (i, s) in c.remote_play().sessions().iter().enumerate() {
                let info = lua.create_table()?;
                info.set("UserID", s.user().raw().to_string())?;
                if let Some(name) = s.client_name() {
                    info.set("ClientName", name)?;
                }
                if let Some((w, h)) = s.client_resolution() {
                    info.set("Width", w as i64)?;
                    info.set("Height", h as i64)?;
                }
                arr.set(i + 1, info)?;
            }
            Ok(arr)
        })?,
    )?;
    Ok(t)
}

fn build_overlay(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "Show",
        lua.create_function(|_, dialog: String| -> mlua::Result<()> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            c.friends().activate_game_overlay(&dialog);
            Ok(())
        })?,
    )?;
    t.set(
        "ShowFriends",
        lua.create_function(|_, _: ()| -> mlua::Result<()> {
            if let Some(c) = CLIENT.get() {
                c.friends().activate_game_overlay("Friends");
            }
            Ok(())
        })?,
    )?;
    t.set(
        "ShowAchievements",
        lua.create_function(|_, _: ()| -> mlua::Result<()> {
            if let Some(c) = CLIENT.get() {
                c.friends().activate_game_overlay("Achievements");
            }
            Ok(())
        })?,
    )?;
    t.set(
        "ShowSettings",
        lua.create_function(|_, _: ()| -> mlua::Result<()> {
            if let Some(c) = CLIENT.get() {
                c.friends().activate_game_overlay("Settings");
            }
            Ok(())
        })?,
    )?;
    t.set(
        "ShowURL",
        lua.create_function(|_, url: String| -> mlua::Result<()> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            c.friends().activate_game_overlay_to_web_page(&url);
            Ok(())
        })?,
    )?;
    t.set(
        "ShowStore",
        lua.create_function(
            |_, (app_id, mode): (Option<u32>, Option<String>)| -> mlua::Result<()> {
                let c = CLIENT
                    .get()
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
                let id = app_id.unwrap_or_else(|| {
                    std::env::var("RUZIT_STEAM_APPID")
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(DEFAULT_APP_ID)
                });
                let flag = match mode.as_deref().unwrap_or("None") {
                    "AddToCart" => OverlayToStoreFlag::AddToCart,
                    "AddToCartAndShow" => OverlayToStoreFlag::AddToCartAndShow,
                    _ => OverlayToStoreFlag::None,
                };
                c.friends().activate_game_overlay_to_store(AppId(id), flag);
                Ok(())
            },
        )?,
    )?;
    t.set(
        "ShowUser",
        lua.create_function(
            |_, (id_str, dialog): (String, Option<String>)| -> mlua::Result<()> {
                let c = CLIENT
                    .get()
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
                let raw: u64 = id_str.parse().map_err(|e| {
                    mlua::Error::RuntimeError(format!("Overlay.ShowUser: bad id: {e}"))
                })?;
                let dlg = dialog.as_deref().unwrap_or("steamid");
                c.friends()
                    .activate_game_overlay_to_user(dlg, SteamId::from_raw(raw));
                Ok(())
            },
        )?,
    )?;
    t.set(
        "ShowInvite",
        lua.create_function(|_, lobby_id: String| -> mlua::Result<()> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let raw: u64 = lobby_id.parse().map_err(|e| {
                mlua::Error::RuntimeError(format!("Overlay.ShowInvite: bad id: {e}"))
            })?;
            c.friends().activate_invite_dialog(LobbyId::from_raw(raw));
            Ok(())
        })?,
    )?;
    Ok(t)
}

fn build_user(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.set(
        "__index",
        lua.create_function(|lua, (_t, key): (Table, String)| -> mlua::Result<Value> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            match key.as_str() {
                "ID" => Ok(Value::String(
                    lua.create_string(&c.user().steam_id().raw().to_string())?,
                )),
                "Name" => Ok(Value::String(lua.create_string(&c.friends().name())?)),
                "Level" => Ok(Value::Integer(c.user().level() as i64 as i32)),
                _ => Ok(Value::Nil),
            }
        })?,
    )?;
    t.set_metatable(Some(meta));
    t.set(
        "GetAvatar",
        lua.create_function(|lua, size: Option<String>| -> mlua::Result<Value> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let me = c.user().steam_id();
            let friend = c.friends().get_friend(me);
            avatar_to_image(lua, &friend, size.as_deref().unwrap_or("medium"))
        })?,
    )?;
    t.set(
        "GetFriends",
        lua.create_function(|lua, _: ()| -> mlua::Result<Table> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let arr = lua.create_table()?;
            for (i, f) in c
                .friends()
                .get_friends(FriendFlags::IMMEDIATE)
                .iter()
                .enumerate()
            {
                let info = lua.create_table()?;
                info.set("ID", f.id().raw().to_string())?;
                info.set("Name", f.name())?;
                info.set(
                    "State",
                    match f.state() {
                        FriendState::Offline => "Offline",
                        FriendState::Online => "Online",
                        FriendState::Busy => "Busy",
                        FriendState::Away => "Away",
                        FriendState::Snooze => "Snooze",
                        FriendState::LookingToTrade => "LookingToTrade",
                        FriendState::LookingToPlay => "LookingToPlay",
                    },
                )?;
                arr.set(i + 1, info)?;
            }
            Ok(arr)
        })?,
    )?;
    t.set(
        "GetFriendAvatar",
        lua.create_function(
            |lua, (id_str, size): (String, Option<String>)| -> mlua::Result<Value> {
                let c = CLIENT
                    .get()
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
                let raw: u64 = id_str.parse().map_err(|e| {
                    mlua::Error::RuntimeError(format!("GetFriendAvatar: bad id: {e}"))
                })?;
                let friend = c.friends().get_friend(SteamId::from_raw(raw));
                avatar_to_image(lua, &friend, size.as_deref().unwrap_or("medium"))
            },
        )?,
    )?;
    t.set(
        "GetAvatarAsync",
        lua.create_function(
            |lua, (id_str, size): (String, Option<String>)| -> mlua::Result<Table> {
                let c = CLIENT
                    .get()
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
                let raw: u64 = id_str.parse().map_err(|e| {
                    mlua::Error::RuntimeError(format!("GetAvatarAsync: bad id: {e}"))
                })?;
                let size = size.unwrap_or_else(|| "medium".into());
                let signal_table = signal::new_instance(lua)?;
                let friend = c.friends().get_friend(SteamId::from_raw(raw));
                let cached = avatar_to_image(lua, &friend, &size)?;
                if !matches!(cached, Value::Nil) {
                    let mut args = MultiValue::new();
                    args.push_back(cached);
                    signal::fire(lua, &signal_table, args)?;
                    return Ok(signal_table);
                }
                PENDING_AVATARS.with(|m| {
                    m.borrow_mut()
                        .entry(raw)
                        .or_default()
                        .push((size, signal_table.clone()));
                });
                let still_loading = c
                    .friends()
                    .request_user_information(SteamId::from_raw(raw), false);
                if !still_loading {
                    let pending = PENDING_AVATARS.with(|m| m.borrow_mut().remove(&raw));
                    if let Some(entries) = pending {
                        let friend = c.friends().get_friend(SteamId::from_raw(raw));
                        for (sz, sig) in entries {
                            let mut args = MultiValue::new();
                            args.push_back(avatar_to_image(lua, &friend, &sz)?);
                            signal::fire(lua, &sig, args)?;
                        }
                    }
                }
                Ok(signal_table)
            },
        )?,
    )?;
    t.set(
        "GetFriendInfo",
        lua.create_function(|lua, id_str: String| -> mlua::Result<Table> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let raw: u64 = id_str
                .parse()
                .map_err(|e| mlua::Error::RuntimeError(format!("GetFriendInfo: bad id: {e}")))?;
            let friend = c.friends().get_friend(SteamId::from_raw(raw));
            let info = lua.create_table()?;
            info.set("ID", friend.id().raw().to_string())?;
            info.set("Name", friend.name())?;
            if let Some(nick) = friend.nick_name() {
                info.set("Nickname", nick)?;
            }
            info.set(
                "State",
                match friend.state() {
                    FriendState::Offline => "Offline",
                    FriendState::Online => "Online",
                    FriendState::Busy => "Busy",
                    FriendState::Away => "Away",
                    FriendState::Snooze => "Snooze",
                    FriendState::LookingToTrade => "LookingToTrade",
                    FriendState::LookingToPlay => "LookingToPlay",
                },
            )?;
            if let Some(game) = friend.game_played() {
                let g = lua.create_table()?;
                g.set("AppID", game.game.app_id().0)?;
                info.set("Game", g)?;
            }
            Ok(info)
        })?,
    )?;
    t.set(
        "RequestInfo",
        lua.create_function(
            |_, (id_str, name_only): (String, Option<bool>)| -> mlua::Result<bool> {
                let c = CLIENT
                    .get()
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
                let raw: u64 = id_str
                    .parse()
                    .map_err(|e| mlua::Error::RuntimeError(format!("RequestInfo: bad id: {e}")))?;
                Ok(c.friends()
                    .request_user_information(SteamId::from_raw(raw), name_only.unwrap_or(false)))
            },
        )?,
    )?;
    t.set(
        "InviteToGame",
        lua.create_function(
            |_, (id_str, connect_str): (String, String)| -> mlua::Result<()> {
                let c = CLIENT
                    .get()
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
                let raw: u64 = id_str
                    .parse()
                    .map_err(|e| mlua::Error::RuntimeError(format!("InviteToGame: bad id: {e}")))?;
                let _friend = c.friends().get_friend(SteamId::from_raw(raw));
                c.friends()
                    .activate_game_overlay_to_user("steamid", SteamId::from_raw(raw));
                let _ = connect_str;
                Ok(())
            },
        )?,
    )?;
    t.set(
        "LoggedOn",
        lua.create_function(|_, _: ()| -> mlua::Result<bool> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.user().logged_on())
        })?,
    )?;
    t.set(
        "AccountID",
        lua.create_function(|_, _: ()| -> mlua::Result<u32> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            Ok(c.user().steam_id().account_id().raw())
        })?,
    )?;
    Ok(t)
}

fn avatar_to_image(
    lua: &Lua,
    friend: &steamworks::Friend<steamworks::ClientManager>,
    size: &str,
) -> mlua::Result<Value> {
    let (rgba_opt, w, h) = match size {
        "small" | "Small" => (friend.small_avatar(), 32u32, 32u32),
        "large" | "Large" => (friend.large_avatar(), 184u32, 184u32),
        _ => (friend.medium_avatar(), 64u32, 64u32),
    };
    match rgba_opt {
        Some(rgba) => crate::libs::asset::make_image_from_rgba(
            lua,
            w,
            h,
            rgba,
            format!("<steam:avatar:{}:{}>", friend.id().raw(), size),
        ),
        None => Ok(Value::Nil),
    }
}

fn build_achievements(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "Unlock",
        lua.create_function(|_, name: String| -> mlua::Result<()> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            c.user_stats()
                .achievement(&name)
                .set()
                .map_err(|e| mlua::Error::RuntimeError(format!("Unlock: {e:?}")))?;
            Ok(())
        })?,
    )?;
    t.set(
        "Clear",
        lua.create_function(|_, name: String| -> mlua::Result<()> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            c.user_stats()
                .achievement(&name)
                .clear()
                .map_err(|e| mlua::Error::RuntimeError(format!("Clear: {e:?}")))?;
            Ok(())
        })?,
    )?;
    t.set(
        "IsUnlocked",
        lua.create_function(|_, name: String| -> mlua::Result<bool> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            c.user_stats()
                .achievement(&name)
                .get()
                .map_err(|e| mlua::Error::RuntimeError(format!("IsUnlocked: {e:?}")))
        })?,
    )?;
    t.set(
        "Store",
        lua.create_function(|_, _: ()| -> mlua::Result<()> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            c.user_stats()
                .store_stats()
                .map_err(|e| mlua::Error::RuntimeError(format!("Store: {e:?}")))?;
            Ok(())
        })?,
    )?;
    Ok(t)
}

fn build_stats(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "SetInt",
        lua.create_function(|_, (name, value): (String, i32)| -> mlua::Result<()> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            c.user_stats()
                .set_stat_i32(&name, value)
                .map_err(|e| mlua::Error::RuntimeError(format!("SetInt: {e:?}")))?;
            Ok(())
        })?,
    )?;
    t.set(
        "SetFloat",
        lua.create_function(|_, (name, value): (String, f32)| -> mlua::Result<()> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            c.user_stats()
                .set_stat_f32(&name, value)
                .map_err(|e| mlua::Error::RuntimeError(format!("SetFloat: {e:?}")))?;
            Ok(())
        })?,
    )?;
    t.set(
        "GetInt",
        lua.create_function(|_, name: String| -> mlua::Result<i32> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            c.user_stats()
                .get_stat_i32(&name)
                .map_err(|e| mlua::Error::RuntimeError(format!("GetInt: {e:?}")))
        })?,
    )?;
    t.set(
        "GetFloat",
        lua.create_function(|_, name: String| -> mlua::Result<f32> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            c.user_stats()
                .get_stat_f32(&name)
                .map_err(|e| mlua::Error::RuntimeError(format!("GetFloat: {e:?}")))
        })?,
    )?;
    t.set(
        "Store",
        lua.create_function(|_, _: ()| -> mlua::Result<()> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            c.user_stats()
                .store_stats()
                .map_err(|e| mlua::Error::RuntimeError(format!("Store: {e:?}")))?;
            Ok(())
        })?,
    )?;
    Ok(t)
}

fn build_lobby(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "Create",
        lua.create_function(
            |lua, (max_members, kind): (Option<i32>, Option<String>)| -> mlua::Result<Table> {
                let c = CLIENT
                    .get()
                    .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
                let max = max_members.unwrap_or(8);
                let lobby_type = match kind.as_deref().unwrap_or("Public") {
                    "Private" => LobbyType::Private,
                    "FriendsOnly" => LobbyType::FriendsOnly,
                    "Invisible" => LobbyType::Invisible,
                    _ => LobbyType::Public,
                };
                let signal_table = signal::new_instance(lua)?;
                let pending_index = PENDING_LOBBY_CREATES.with(|p| {
                    let mut p = p.borrow_mut();
                    p.push(signal_table.clone());
                    p.len() - 1
                });
                let evq = events();
                c.matchmaking()
                    .create_lobby(lobby_type, max as u32, move |result| {
                        let r = match result {
                            Ok(id) => LobbyResult {
                                success: true,
                                lobby_id: id.raw(),
                                pending_index,
                            },
                            Err(_) => LobbyResult {
                                success: false,
                                lobby_id: 0,
                                pending_index,
                            },
                        };
                        evq.lock().unwrap().lobby_created.push(r);
                    });
                Ok(signal_table)
            },
        )?,
    )?;

    t.set(
        "Join",
        lua.create_function(|lua, id_str: String| -> mlua::Result<Table> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let raw: u64 = id_str
                .parse()
                .map_err(|e| mlua::Error::RuntimeError(format!("Lobby.Join: bad id: {e}")))?;
            let signal_table = signal::new_instance(lua)?;
            PENDING_LOBBY_JOINS.with(|p| p.borrow_mut().insert(raw, signal_table.clone()));
            let evq = events();
            c.matchmaking()
                .join_lobby(LobbyId::from_raw(raw), move |result| {
                    let r = match result {
                        Ok(id) => LobbyResult {
                            success: true,
                            lobby_id: id.raw(),
                            pending_index: 0,
                        },
                        Err(_) => LobbyResult {
                            success: false,
                            lobby_id: raw,
                            pending_index: 0,
                        },
                    };
                    evq.lock().unwrap().lobby_joined.push(r);
                });
            Ok(signal_table)
        })?,
    )?;

    t.set(
        "List",
        lua.create_function(|lua, _: ()| -> mlua::Result<Table> {
            let c = CLIENT
                .get()
                .ok_or_else(|| mlua::Error::RuntimeError("Steam not initialized".into()))?;
            let signal_table = signal::new_instance(lua)?;
            let pending_index = PENDING_LOBBY_LISTS.with(|p| {
                let mut p = p.borrow_mut();
                p.push(signal_table.clone());
                p.len() - 1
            });
            let evq = events();
            c.matchmaking().request_lobby_list(move |result| {
                let r = match result {
                    Ok(ids) => LobbyListResult {
                        success: true,
                        lobby_ids: ids.into_iter().map(|i| i.raw()).collect(),
                        pending_index,
                    },
                    Err(_) => LobbyListResult {
                        success: false,
                        lobby_ids: Vec::new(),
                        pending_index,
                    },
                };
                evq.lock().unwrap().lobby_list.push(r);
            });
            Ok(signal_table)
        })?,
    )?;

    Ok(t)
}

fn build_server(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "Start",
        lua.create_function(|lua, opts: Table| -> mlua::Result<AnyUserData> {
            if SERVER.get().is_some() {
                return Err(mlua::Error::RuntimeError(
                    "Steam server already started".into(),
                ));
            }
            let game_port: u16 = opts.get("port").unwrap_or(27015);
            let query_port: u16 = opts.get("queryPort").unwrap_or(27016);
            let mode_str: String = opts
                .get("mode")
                .unwrap_or_else(|_| "NoAuthentication".into());
            let version: String = opts.get("version").unwrap_or_else(|_| "1.0.0.0".into());
            let mode = match mode_str.as_str() {
                "Authentication" => ServerMode::Authentication,
                "AuthenticationAndSecure" => ServerMode::AuthenticationAndSecure,
                _ => ServerMode::NoAuthentication,
            };
            let (server, single) = Server::init(
                Ipv4Addr::new(0, 0, 0, 0),
                game_port,
                query_port,
                mode,
                &version,
            )
            .map_err(|e| mlua::Error::RuntimeError(format!("Server.Start: {e}")))?;

            if let Ok(n) = opts.get::<String>("name") {
                server.set_server_name(&n);
            }
            if let Ok(m) = opts.get::<String>("map") {
                server.set_map_name(&m);
            }
            if let Ok(m) = opts.get::<i32>("max") {
                server.set_max_players(m);
            }

            register_server_callbacks(&server);
            let client_authed = signal::new_instance(lua)?;
            let disconnected = signal::new_instance(lua)?;
            let connected = signal::new_instance(lua)?;
            let connect_failure = signal::new_instance(lua)?;
            let closed = signal::new_instance(lua)?;
            SERVER_SIGNALS.with(|s| {
                *s.borrow_mut() = Some(ServerSignals {
                    client_authed,
                    disconnected,
                    connected,
                    connect_failure,
                    closed,
                });
            });

            let _ = SERVER.set(server);
            SERVER_SINGLE.with(|c| *c.borrow_mut() = Some(single));
            lua.create_userdata(ServerHandle)
        })?,
    )?;
    Ok(t)
}
