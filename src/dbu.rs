//! DBU's results pages for the 1. division, which ESPN does not have.
//!
//! DBU has no open API, so this reads the public HTML at dbu.dk/resultater:
//!
//! - the row search for this season's national senior men's rows, to find the
//!   1. division (`LIGA` in the name, e.g. "Betinia LIGA - Grundspil 2026/27")
//! - the row's pool and its team list, for the team picker
//! - a club's page, which lists every pool its teams play in this season,
//!   so the league, its second half and Betano Pokalen are found without
//!   knowing their ids
//! - a team's fixtures in one pool
//!
//! Team ids (`hold/3363_...`) and club ids (`klub/69`) stay the same from
//! season to season; pool ids do not. Times are Danish local time.

use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Europe::Copenhagen;
use regex::Regex;
use std::sync::LazyLock;

use crate::espn::{Match, Team};

pub const BASE_URL: &str = "https://www.dbu.dk";
/// The 1. division in [`crate::espn::LEAGUES`].
pub const LEAGUE: &str = "dbu.1div";

/// The row search: national (union 1), men (gender 0), senior (division 1), this season.
pub const ROW_SEARCH: &str = "/resultater/raekkesoeg/?unionid=1&genderid=0&divisionid=1&season=0";

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}

static TAG: LazyLock<Regex> = LazyLock::new(|| re(r"<[^>]+>"));
static TEAM_ID: LazyLock<Regex> = LazyLock::new(|| re(r"/resultater/hold/(\d+)_"));
static IMG: LazyLock<Regex> = LazyLock::new(|| re(r#"<img src="([^"]+)""#));
static SCORE: LazyLock<Regex> = LazyLock::new(|| re(r"\d+\s*-\s*\d+"));

fn text(html: &str) -> String {
    let no_tags = TAG.replace_all(html, " ");
    let s = no_tags
        .replace("&amp;", "&")
        .replace("&nbsp;", " ")
        .replace("&#xF8;", "ø")
        .replace("&#xE6;", "æ")
        .replace("&#xE5;", "å")
        .replace("&#xD8;", "Ø")
        .replace("&#xC6;", "Æ")
        .replace("&#xC5;", "Å")
        .replace("&#xFC;", "ü")
        .replace("&#xE9;", "é")
        .replace("&#x27;", "'")
        .replace("&quot;", "\"");
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The rows of the 1. division this season, from the row search: their id and name.
/// The sponsor's name changes, so it is the row with `LIGA` in capitals (3F
/// Superliga and Futsal Liga do not have that).
pub fn league_rows(html: &str) -> Vec<(String, String)> {
    let re = Regex::new(r#"href="/resultater/Raekke/(\d+)"[^>]*>([^<]*)<"#).unwrap();
    re.captures_iter(html)
        .map(|c| (c[1].to_string(), text(&c[2])))
        .filter(|(_, name)| name.split_whitespace().any(|w| w == "LIGA"))
        .collect()
}

/// The pool id at the end of a row's address, after DBU's redirect.
pub fn pool_from_url(url: &str) -> Option<String> {
    Regex::new(r"/pulje/(\d+)").unwrap().captures(url).map(|c| c[1].to_string())
}

/// The teams in a pool's team list (`/resultater/pulje/{pool}/holdoversigt`), by name.
pub fn parse_team_list(html: &str) -> Vec<Team> {
    let re = Regex::new(r#"(?s)/resultater/hold/(\d+)_\d+/?'"\s*>\s*<div class="sr--pool--team-list--team--name">\s*<h3>([^<]*)</h3>"#).unwrap();
    let mut teams: Vec<Team> = re
        .captures_iter(html)
        .map(|c| Team { id: c[1].to_string(), name: text(&c[2]), league: LEAGUE.to_string(), logo: None, club: None })
        .collect();
    teams.sort_by_key(|t| t.name.to_lowercase());
    teams.dedup_by(|a, b| a.id == b.id);
    teams
}

/// The club on a team's page.
pub fn club_of(html: &str) -> Option<String> {
    Regex::new(r"/resultater/klub/(\d+)").unwrap().captures(html).map(|c| c[1].to_string())
}

/// The pools a team plays in this season, from its club's page: the pool
/// id and the competition's name, e.g. "Betano Pokalen 2026/27".
pub fn team_pools(club_html: &str, team_id: &str) -> Vec<(String, String)> {
    let re = Regex::new(r#"href="/resultater/hold/(\d+)_(\d+)/kampprogram"[^>]*>([^<]*)<"#).unwrap();
    let mut pools: Vec<(String, String)> = vec![];
    for c in re.captures_iter(club_html) {
        if &c[1] != team_id || pools.iter().any(|(p, _)| p == &c[2]) {
            continue;
        }
        pools.push((c[2].to_string(), text(&c[3])));
    }
    pools
}

/// "Betinia LIGA - Grundspil 2026/27" is the competition "Betinia LIGA" and the round "Grundspil".
pub fn split_competition(name: &str) -> (String, Option<String>) {
    let name = Regex::new(r"\s+\d{4}(/\d{2,4}|-\d{2,4})?$").unwrap().replace(name.trim(), "").to_string();
    match name.split_once(" - ") {
        Some((c, r)) => (c.trim().to_string(), Some(r.trim().to_string())),
        None => (name, None),
    }
}

struct Side {
    id: String,
    name: String,
    logo: Option<String>,
}

fn side(cell: &str) -> Side {
    let id = TEAM_ID.captures(cell).map(|c| c[1].to_string()).unwrap_or_default();
    let logo = IMG.captures(cell).map(|c| c[1].to_string());
    Side { id, name: text(cell), logo }
}

/// A team's matches in one pool, from `/resultater/hold/{team}_{pool}/kampprogram`.
pub fn parse_fixtures(html: &str, team_id: &str, competition: &str) -> Vec<Match> {
    let (competition, round) = split_competition(competition);
    // the columns differ between pools (TV is not always there), so go by the headings
    let heads: Vec<String> = Regex::new(r"(?s)<thead.*?</thead>")
        .unwrap()
        .find(html)
        .map(|h| Regex::new(r"(?s)<th[^>]*>(.*?)</th>").unwrap().captures_iter(h.as_str()).map(|c| text(&c[1])).collect())
        .unwrap_or_default();
    let col = |name: &str| heads.iter().position(|h| h == name);
    let (Some(nr), Some(date), Some(home), Some(away)) = (col("Kampnr"), col("Dato"), col("Hjemme"), col("Ude")) else {
        return vec![];
    };
    let (time, venue, tv, result) = (col("Tid"), col("Spillested"), col("TV"), col("Resultat"));

    let row_re = Regex::new(r#"(?s)<tr[^>]*MatchProgramMatchClick[^>]*>(.*?)</tr>"#).unwrap();
    let cell_re = Regex::new(r#"(?s)<td class="([^"]*)"[^>]*>(.*?)</td>"#).unwrap();
    let date_re = Regex::new(r"(\d{2})-(\d{2}) (\d{4})").unwrap();
    let mut out: Vec<Match> = vec![];
    for row in row_re.captures_iter(html) {
        let cells: Vec<&str> = cell_re
            .captures_iter(&row[1])
            .filter(|c| !c[1].contains("only-on-mobile"))
            .map(|c| c.get(2).unwrap().as_str())
            .collect();
        let get = |i: Option<usize>| i.and_then(|i| cells.get(i)).copied().unwrap_or("");
        let id = text(get(Some(nr)));
        let date_text = text(get(Some(date)));
        let Some(d) = date_re.captures(&date_text) else { continue };
        let Some(day) = NaiveDate::from_ymd_opt(d[3].parse().unwrap_or(0), d[2].parse().unwrap_or(0), d[1].parse().unwrap_or(0)) else {
            continue;
        };
        let clock = NaiveTime::parse_from_str(&text(get(time)), "%H:%M").ok();
        let local = day.and_time(clock.unwrap_or(NaiveTime::from_hms_opt(12, 0, 0).unwrap()));
        let Some(start) = Copenhagen.from_local_datetime(&local).earliest().map(|t| t.with_timezone(&Utc)) else { continue };
        let (h, a) = (side(get(Some(home))), side(get(Some(away))));
        let we_are_home = h.id == team_id;
        if !we_are_home && a.id != team_id {
            continue;
        }
        let them = if we_are_home { &a } else { &h };
        let played = SCORE.is_match(&text(get(result)));
        let tv_name = text(get(tv));
        if out.iter().any(|m| m.id == id) {
            continue;
        }
        out.push(Match {
            id,
            start,
            time_confirmed: clock.is_some(),
            competition: competition.clone(),
            round: round.clone(),
            home: we_are_home,
            home_team: h.name.clone(),
            away_team: a.name.clone(),
            opponent: them.name.clone(),
            opponent_id: them.id.clone(),
            opponent_logo: them.logo.clone(),
            venue: Some(text(get(venue))).filter(|v| !v.is_empty()),
            city: None,
            tv: Some(tv_name).filter(|v| !v.is_empty()),
            state: if played { "post" } else { "pre" }.to_string(),
        });
    }
    out
}

/// The time of a match, for tests.
pub fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
    Copenhagen.with_ymd_and_hms(y, m, d, h, min, 0).unwrap().with_timezone(&Utc)
}
