//! アプリ全体で使うデータ型と定数。

pub const POSITIONS: [&str; 5] = ["top", "jungle", "middle", "bottom", "utility"];
pub const LANES: [&str; 4] = ["top", "jungle", "middle", "bottom"];

pub const TIERS: [&str; 10] = [
    "IRON", "BRONZE", "SILVER", "GOLD", "PLATINUM", "EMERALD", "DIAMOND", "MASTER", "GRANDMASTER",
    "CHALLENGER",
];

pub fn tier_label(tier: &str) -> &str {
    match tier {
        "ALL" => "全ランク",
        "IRON" => "アイアン",
        "BRONZE" => "ブロンズ",
        "SILVER" => "シルバー",
        "GOLD" => "ゴールド",
        "PLATINUM" => "プラチナ",
        "EMERALD" => "エメラルド",
        "DIAMOND" => "ダイヤモンド",
        "MASTER" => "マスター",
        "GRANDMASTER" => "グランドマスター",
        "CHALLENGER" => "チャレンジャー",
        other => other,
    }
}

pub fn position_label(pos: &str) -> &'static str {
    match pos {
        "top" => "TOP",
        "jungle" => "JG",
        "middle" => "MID",
        "bottom" => "ADC",
        "utility" => "SUP",
        _ => "?",
    }
}

pub fn lane_label(lane: &str) -> &'static str {
    match lane {
        "top" => "TOP",
        "jungle" => "JG",
        "middle" => "MID",
        "bottom" => "BOT",
        _ => "?",
    }
}

/// 表記ゆれのあるポジション文字列を POSITIONS のどれかに直す。不明なら None。
pub fn normalize_position(value: Option<&str>) -> Option<&'static str> {
    let v = value?.trim().to_lowercase();
    Some(match v.as_str() {
        "top" => "top",
        "jungle" | "jg" => "jungle",
        "middle" | "mid" => "middle",
        "bottom" | "bot" | "adc" => "bottom",
        "utility" | "support" | "sup" => "utility",
        _ => return None,
    })
}

pub fn as_position(s: &str) -> Option<&'static str> {
    POSITIONS.iter().copied().find(|p| *p == s)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Player {
    pub champion: Option<String>,
    pub position: Option<String>,
    pub has_smite: bool,
    pub hovering: bool,
    pub is_local: bool,
}

#[derive(Debug, Clone)]
pub struct GameState {
    pub phase: String,
    pub status: String,
    pub side: Option<String>,
    pub ally: Vec<Player>,
    pub enemy: Vec<Player>,
}

impl Default for GameState {
    fn default() -> Self {
        GameState {
            phase: "disconnected".into(),
            status: String::new(),
            side: None,
            ally: vec![],
            enemy: vec![],
        }
    }
}

#[derive(Debug, Clone)]
pub struct Placement {
    pub player: Player,
    pub position: &'static str,
    pub confidence: f64,
    pub fixed: bool,
}
