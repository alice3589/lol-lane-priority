//! 状態の取得元。LoL クライアント（LCU API / Live Client Data API）と、動作確認用のモック。
//!
//! LCU の接続情報は次の順で探す。
//!   1. 設定で指定された lockfile
//!   2. よくあるインストール先の lockfile
//!   3. 起動中の LeagueClientUx.exe のコマンドライン（--app-port / --remoting-auth-token）
use crate::champions::ChampionCatalog;
use crate::models::{normalize_position, GameState, Player};
use base64::Engine;
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const LOCKFILE_CANDIDATES: [&str; 5] = [
    r"C:\Riot Games\League of Legends\lockfile",
    r"D:\Riot Games\League of Legends\lockfile",
    r"E:\Riot Games\League of Legends\lockfile",
    r"C:\Program Files\Riot Games\League of Legends\lockfile",
    r"C:\Program Files (x86)\Riot Games\League of Legends\lockfile",
];
const SMITE_SPELL_ID: i64 = 11;
const ALL_GAME_DATA_URL: &str = "https://127.0.0.1:2999/liveclientdata/allgamedata";
/// 未接続のとき、クライアントを探し直す間隔（プロセス検索は重いので毎回はやらない）
const SEARCH_INTERVAL: Duration = Duration::from_secs(5);

/// 自己署名証明書を受け入れる HTTP クライアント（127.0.0.1 専用）
fn insecure_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(ureq::tls::TlsConfig::builder().disable_verification(true).build())
        .timeout_global(Some(timeout))
        .build()
        .into()
}

#[derive(Debug, Clone, PartialEq)]
pub struct LcuCredentials {
    pub port: u16,
    pub password: String,
    pub protocol: String,
}

impl LcuCredentials {
    fn base_url(&self) -> String {
        format!("{}://127.0.0.1:{}", self.protocol, self.port)
    }
}

/// lockfile の中身（例: LeagueClient:1234:54321:abcdef:https）を読む。
pub fn parse_lockfile(text: &str) -> Option<LcuCredentials> {
    let parts: Vec<&str> = text.trim().split(':').collect();
    if parts.len() < 5 {
        return None;
    }
    Some(LcuCredentials { port: parts[2].parse().ok()?, password: parts[3].into(), protocol: parts[4].into() })
}

fn arg_value<'a>(cmdline: &'a str, key: &str, valid: impl Fn(char) -> bool) -> Option<&'a str> {
    let start = cmdline.find(key)? + key.len();
    let rest = &cmdline[start..];
    let end = rest.find(|c: char| !valid(c)).unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

pub fn parse_commandline(cmdline: &str) -> Option<LcuCredentials> {
    let port = arg_value(cmdline, "--app-port=", |c| c.is_ascii_digit())?;
    let token = arg_value(cmdline, "--remoting-auth-token=", |c| c.is_alphanumeric() || c == '_' || c == '-')?;
    Some(LcuCredentials { port: port.parse().ok()?, password: token.into(), protocol: "https".into() })
}

#[cfg(windows)]
fn process_commandline() -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "(Get-CimInstance Win32_Process -Filter \"Name='LeagueClientUx.exe'\").CommandLine",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(not(windows))]
fn process_commandline() -> Option<String> {
    None
}

pub fn find_credentials(lockfile_path: &str) -> Option<LcuCredentials> {
    let custom = (!lockfile_path.is_empty()).then_some(lockfile_path);
    for p in custom.into_iter().chain(LOCKFILE_CANDIDATES) {
        if let Some(c) = std::fs::read_to_string(p).ok().and_then(|t| parse_lockfile(&t)) {
            return Some(c);
        }
    }
    parse_commandline(&process_commandline()?)
}

pub struct LcuClient {
    pub lockfile_path: String,
    creds: Option<LcuCredentials>,
    agent: ureq::Agent,
    last_search: Option<Instant>,
}

impl LcuClient {
    pub fn new(lockfile_path: &str) -> Self {
        LcuClient { lockfile_path: lockfile_path.into(), creds: None, agent: insecure_agent(Duration::from_secs(2)), last_search: None }
    }

    /// 接続情報を捨て、次回すぐに探し直す（設定変更時など）。
    pub fn reset(&mut self) {
        self.creds = None;
        self.last_search = None;
    }

    pub fn ensure_connected(&mut self) -> bool {
        if self.creds.is_none() {
            if self.last_search.is_some_and(|t| t.elapsed() < SEARCH_INTERVAL) {
                return false;
            }
            self.last_search = Some(Instant::now());
            self.creds = find_credentials(&self.lockfile_path);
        }
        self.creds.is_some()
    }

    /// GET して JSON を返す。404 などは None。接続できなければ接続情報を捨てて None。
    pub fn get(&mut self, path: &str) -> Option<Value> {
        if !self.ensure_connected() {
            return None;
        }
        let creds = self.creds.as_ref()?;
        let auth = base64::engine::general_purpose::STANDARD.encode(format!("riot:{}", creds.password));
        match self.agent.get(format!("{}{path}", creds.base_url())).header("Authorization", format!("Basic {auth}")).call() {
            Ok(mut r) => r.body_mut().read_json::<Value>().ok(),
            Err(ureq::Error::StatusCode(_)) => None,
            Err(_) => {
                // クライアントが終了した / lockfile が古い → 次回探し直す
                self.creds = None;
                None
            }
        }
    }
}

fn pick_actions(session: &Value) -> HashMap<i64, &Value> {
    let mut picks = HashMap::new();
    for group in session["actions"].as_array().into_iter().flatten() {
        for action in group.as_array().into_iter().flatten() {
            if action["type"] == "pick" {
                if let Some(cell) = action["actorCellId"].as_i64() {
                    picks.insert(cell, action);
                }
            }
        }
    }
    picks
}

fn member_to_player(m: &Value, picks: &HashMap<i64, &Value>, catalog: &ChampionCatalog, local_cell: Option<i64>) -> Player {
    let cell = m["cellId"].as_i64();
    let mut champ_id = m["championId"].as_i64().unwrap_or(0);
    let hovering = if champ_id != 0 {
        cell.and_then(|c| picks.get(&c)).is_some_and(|a| !a["completed"].as_bool().unwrap_or(false))
    } else {
        champ_id = m["championPickIntent"].as_i64().unwrap_or(0);
        champ_id != 0
    };
    let spells = [m["spell1Id"].as_i64(), m["spell2Id"].as_i64()];
    Player {
        champion: catalog.id_by_key(Some(champ_id)).map(String::from),
        position: normalize_position(m["assignedPosition"].as_str()).map(String::from),
        has_smite: spells.contains(&Some(SMITE_SPELL_ID)),
        hovering,
        is_local: cell.is_some() && cell == local_cell,
    }
}

/// /lol-champ-select/v1/session の JSON を GameState に変換する。
pub fn parse_session(session: &Value, catalog: &ChampionCatalog) -> GameState {
    let picks = pick_actions(session);
    let local_cell = session["localPlayerCellId"].as_i64();
    let empty = vec![];
    let my_team = session["myTeam"].as_array().unwrap_or(&empty);
    let their_team = session["theirTeam"].as_array().unwrap_or(&empty);
    let ally = my_team.iter().map(|m| member_to_player(m, &picks, catalog, local_cell)).collect();
    // 敵の assignedPosition は通常空。入っていても信用しすぎないよう推定に任せる
    let enemy = their_team
        .iter()
        .map(|m| Player { position: None, ..member_to_player(m, &picks, catalog, None) })
        .collect();
    let side = match my_team.first().and_then(|m| m["team"].as_i64()) {
        Some(1) => Some("blue".to_string()),
        Some(2) => Some("red".to_string()),
        _ => None,
    };
    GameState { phase: "champ_select".into(), status: "チャンピオンセレクト中".into(), side, ally, enemy }
}

fn has_smite(p: &Value) -> bool {
    p["summonerSpells"].as_object().into_iter().flatten().any(|(_, s)| {
        let raw = format!("{}{}", s["rawDisplayName"].as_str().unwrap_or(""), s["displayName"].as_str().unwrap_or(""));
        raw.to_lowercase().contains("smite") || raw.contains("スマイト")
    })
}

fn player_id(p: &Value) -> &str {
    p["riotId"].as_str().filter(|s| !s.is_empty()).or(p["summonerName"].as_str()).unwrap_or("")
}

/// Live Client Data API（試合中・ポート 2999）の allgamedata を GameState に変換する。
pub fn parse_live(data: &Value, catalog: &ChampionCatalog) -> Option<GameState> {
    let players = data["allPlayers"].as_array().filter(|a| !a.is_empty())?;
    let active = &data["activePlayer"];
    let me = active["riotId"].as_str().filter(|s| !s.is_empty()).or(active["summonerName"].as_str()).unwrap_or("");
    let is_me = |p: &Value| !me.is_empty() && player_id(p) == me;
    // 観戦などで自分が見つからないときは青側を味方として扱う
    let my_team = players.iter().find(|p| is_me(p)).and_then(|p| p["team"].as_str()).unwrap_or("ORDER");

    let (mut ally, mut enemy) = (vec![], vec![]);
    for p in players {
        let champion = catalog
            .resolve(p["rawChampionName"].as_str())
            .or_else(|| catalog.resolve(p["championName"].as_str()))
            .map(String::from);
        let player = Player {
            champion,
            position: normalize_position(p["position"].as_str()).map(String::from),
            has_smite: has_smite(p),
            is_local: is_me(p),
            ..Default::default()
        };
        if p["team"].as_str() == Some(my_team) { ally.push(player) } else { enemy.push(player) }
    }
    let side = match my_team {
        "ORDER" => Some("blue".to_string()),
        "CHAOS" => Some("red".to_string()),
        _ => None,
    };
    Some(GameState { phase: "in_game".into(), status: "試合中（確定した10人で判定）".into(), side, ally, enemy })
}

pub trait Source: Send {
    fn poll(&mut self, catalog: &ChampionCatalog) -> GameState;
    /// 設定変更（lockfile の場所）を反映する。モックでは何もしない。
    fn set_lockfile(&mut self, _path: &str) {}
}

pub struct ClientSource {
    pub lcu: LcuClient,
    live: ureq::Agent,
    last_select: Option<GameState>,
}

impl ClientSource {
    pub fn new(lockfile_path: &str) -> Self {
        ClientSource { lcu: LcuClient::new(lockfile_path), live: insecure_agent(Duration::from_secs(1)), last_select: None }
    }

    fn fetch_live(&self) -> Option<Value> {
        self.live.get(ALL_GAME_DATA_URL).call().ok()?.body_mut().read_json().ok()
    }
}

fn status(phase: &str, text: impl Into<String>) -> GameState {
    GameState { phase: phase.into(), status: text.into(), ..Default::default() }
}

impl Source for ClientSource {
    fn poll(&mut self, catalog: &ChampionCatalog) -> GameState {
        // 1. 試合中なら Live Client Data API が一番確実
        if let Some(state) = self.fetch_live().and_then(|d| parse_live(&d, catalog)) {
            return state;
        }
        // 2. クライアント（LCU）
        if !self.lcu.ensure_connected() {
            return status("disconnected", "LoL クライアントが見つかりません（起動を待っています）");
        }
        let Some(phase) = self.lcu.get("/lol-gameflow/v1/gameflow-phase").and_then(|v| v.as_str().map(String::from)) else {
            return status("disconnected", "LoL クライアントに接続できません（再接続を待っています）");
        };
        let in_game = matches!(phase.as_str(), "GameStart" | "InProgress" | "Reconnect");
        if phase == "ChampSelect" {
            return match self.lcu.get("/lol-champ-select/v1/session") {
                Some(session) => {
                    let state = parse_session(&session, catalog);
                    self.last_select = Some(state.clone());
                    state
                }
                None => status("idle", "チャンピオンセレクトの情報を取得中…"),
            };
        }
        if in_game {
            if let Some(last) = &self.last_select {
                // ロード画面中：チャンセレの最終状態を出し続ける
                return GameState {
                    phase: "loading".into(),
                    status: "ロード中（チャンピオンセレクトの結果で判定）".into(),
                    ..last.clone()
                };
            }
        } else {
            self.last_select = None;
        }
        status("idle", format!("待機中（{phase}）"))
    }

    fn set_lockfile(&mut self, path: &str) {
        self.lcu.lockfile_path = path.into();
        self.lcu.reset();
    }
}

// モック: 青サイドのランク戦を想定したチャンセレの進行
const MOCK_ALLY: [(&str, &str, bool); 5] = [
    ("Darius", "top", false),
    ("LeeSin", "jungle", true),
    ("Ahri", "middle", false),
    ("Jinx", "bottom", false),
    ("Lulu", "utility", false),
];
const MOCK_ENEMY: [&str; 5] = ["Kayle", "Nunu", "Orianna", "Draven", "Leona"];
// (味方 or 敵, インデックス) の順でピックが進む（B1 R1 R2 B2 B3 R3 R4 B4 B5 R5）
const MOCK_ORDER: [(char, usize); 10] =
    [('a', 0), ('e', 3), ('e', 4), ('a', 1), ('a', 2), ('e', 1), ('e', 0), ('a', 3), ('a', 4), ('e', 2)];

/// クライアントが無くても画面を確認できるよう、ピックが進んでいく様子を再現する。
#[derive(Default)]
pub struct MockSource {
    tick: usize,
}

impl MockSource {
    const TICKS_PER_PICK: usize = 3;
    const HOLD_TICKS: usize = 40;
}

impl Source for MockSource {
    fn poll(&mut self, _catalog: &ChampionCatalog) -> GameState {
        let total = MOCK_ORDER.len() * Self::TICKS_PER_PICK + Self::HOLD_TICKS;
        let t = self.tick % total;
        self.tick += 1;
        let picked = (t / Self::TICKS_PER_PICK).min(MOCK_ORDER.len());
        let hovering_next = picked < MOCK_ORDER.len();

        let mut ally: Vec<Player> = MOCK_ALLY
            .iter()
            .enumerate()
            .map(|(i, (_, pos, smite))| Player {
                position: Some(pos.to_string()),
                has_smite: *smite,
                is_local: i == 2,
                ..Default::default()
            })
            .collect();
        let mut enemy = vec![Player::default(); MOCK_ENEMY.len()];
        let shown = picked + usize::from(hovering_next);
        for (k, &(side, i)) in MOCK_ORDER[..shown].iter().enumerate() {
            let p = if side == 'a' { &mut ally[i] } else { &mut enemy[i] };
            p.champion = Some(if side == 'a' { MOCK_ALLY[i].0 } else { MOCK_ENEMY[i] }.to_string());
            p.hovering = k == picked;
        }
        let status = if hovering_next { "モック: チャンピオンセレクト中" } else { "モック: ピック完了" };
        GameState { phase: "champ_select".into(), status: status.into(), side: Some("blue".into()), ally, enemy }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lockfile_and_cmdline() {
        let c = parse_lockfile("LeagueClient:1234:54321:abc-DEF_1:https\n").unwrap();
        assert_eq!((c.port, c.password.as_str(), c.protocol.as_str()), (54321, "abc-DEF_1", "https"));
        assert!(parse_lockfile("broken").is_none());
        let c = parse_commandline(r#""LeagueClientUx.exe" "--remoting-auth-token=Xy_z-9" "--app-port=50123""#).unwrap();
        assert_eq!((c.port, c.password.as_str()), (50123, "Xy_z-9"));
        assert!(parse_commandline("--app-port=1").is_none());
    }
}
