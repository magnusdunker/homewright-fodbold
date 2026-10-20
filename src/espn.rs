//! ESPN's JSON for football: the leagues a team can be picked from, the
//! teams in a league, and a team's coming matches.
//!
//! A team's schedule is per competition, so a club's next match is found by
//! asking its league, the league's cups and the three UEFA tournaments, and
//! taking the earliest. ESPN answers 400 for a team that is not in a
//! competition; that is no matches, not an error.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

pub const BASE_URL: &str = "https://site.api.espn.com/apis/site/v2/sports/soccer";

/// A league the user picks teams from.
pub struct League {
    pub slug: &'static str,
    pub name: &'static str,
    /// Where the teams play besides `slug`.
    pub also: &'static [&'static str],
}

const EUROPE: &[&str] = &["uefa.champions", "uefa.europa", "uefa.europa.conf"];

/// `slug` is where the teams are listed. National teams are listed from the
/// Nations League, which has every European team.
pub const LEAGUES: &[League] = &[
    League { slug: "den.1", name: "Superligaen", also: EUROPE },
    League { slug: "eng.1", name: "Premier League", also: &["eng.fa", "eng.league_cup", "uefa.champions", "uefa.europa", "uefa.europa.conf"] },
    League { slug: "eng.2", name: "Championship", also: &["eng.fa", "eng.league_cup"] },
    League { slug: "esp.1", name: "LaLiga", also: &["esp.copa_del_rey", "uefa.champions", "uefa.europa", "uefa.europa.conf"] },
    League { slug: "ger.1", name: "Bundesliga", also: &["ger.dfb_pokal", "uefa.champions", "uefa.europa", "uefa.europa.conf"] },
    League { slug: "ger.2", name: "2. Bundesliga", also: &["ger.dfb_pokal"] },
    League { slug: "ita.1", name: "Serie A", also: &["ita.coppa_italia", "uefa.champions", "uefa.europa", "uefa.europa.conf"] },
    League { slug: "fra.1", name: "Ligue 1", also: &["fra.coupe_de_france", "uefa.champions", "uefa.europa", "uefa.europa.conf"] },
    League { slug: "ned.1", name: "Eredivisie", also: &["ned.cup", "uefa.champions", "uefa.europa", "uefa.europa.conf"] },
    League { slug: "por.1", name: "Primeira Liga", also: &["por.taca.portugal", "uefa.champions", "uefa.europa", "uefa.europa.conf"] },
    League { slug: "sco.1", name: "Scottish Premiership", also: &["sco.tennents", "uefa.champions", "uefa.europa", "uefa.europa.conf"] },
    League { slug: "swe.1", name: "Allsvenskan", also: EUROPE },
    League { slug: "nor.1", name: "Eliteserien", also: EUROPE },
    League { slug: "usa.1", name: "MLS", also: &[] },
    League { slug: "uefa.nations", name: "Landshold", also: &["fifa.worldq.uefa", "fifa.world", "uefa.euroq", "uefa.euro", "fifa.friendly"] },
];

pub fn league(slug: &str) -> Option<&'static League> {
    LEAGUES.iter().find(|l| l.slug == slug)
}

impl League {
    /// Every competition to ask for a team's matches, the league first.
    pub fn competitions(&self) -> Vec<&'static str> {
        std::iter::once(self.slug).chain(self.also.iter().copied()).collect()
    }
}

/// The name shown for a competition.
pub fn competition_name(slug: &str) -> &str {
    match slug {
        "eng.fa" => "FA Cup",
        "eng.league_cup" => "Carabao Cup",
        "esp.copa_del_rey" => "Copa del Rey",
        "ger.dfb_pokal" => "DFB-Pokal",
        "ita.coppa_italia" => "Coppa Italia",
        "fra.coupe_de_france" => "Coupe de France",
        "ned.cup" => "KNVB Beker",
        "por.taca.portugal" => "Taça de Portugal",
        "sco.tennents" => "Scottish Cup",
        "uefa.champions" => "Champions League",
        "uefa.europa" => "Europa League",
        "uefa.europa.conf" => "Conference League",
        "uefa.nations" => "Nations League",
        "fifa.worldq.uefa" => "VM-kvalifikation",
        "fifa.world" => "VM",
        "uefa.euroq" => "EM-kvalifikation",
        "uefa.euro" => "EM",
        "fifa.friendly" => "Venskabskamp",
        s => league(s).map(|l| l.name).unwrap_or(s),
    }
}

/// A team the user follows. Kept in the entry's data.
#[derive(Clone, Debug, PartialEq)]
pub struct Team {
    pub id: String,
    pub name: String,
    /// The league it was picked from.
    pub league: String,
    pub logo: Option<String>,
}

impl Team {
    /// The value in the team picker.
    pub fn key(&self) -> String {
        format!("{}/{}", self.league, self.id)
    }

    pub fn to_json(&self) -> Value {
        json!({ "id": self.id, "name": self.name, "league": self.league, "logo": self.logo })
    }

    pub fn from_json(v: &Value) -> Option<Team> {
        Some(Team {
            id: v["id"].as_str()?.to_string(),
            name: v["name"].as_str()?.to_string(),
            league: v["league"].as_str()?.to_string(),
            logo: v["logo"].as_str().map(String::from),
        })
    }
}

fn logo(team: &Value) -> Option<String> {
    team["logos"].as_array()?.first()?["href"].as_str().map(String::from)
}

/// The teams in a league from `/{league}/teams`, by name.
pub fn parse_teams(league: &str, v: &Value) -> Vec<Team> {
    let mut teams: Vec<Team> = v["sports"][0]["leagues"][0]["teams"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let t = &t["team"];
            Some(Team {
                id: t["id"].as_str()?.to_string(),
                name: t["displayName"].as_str()?.to_string(),
                league: league.to_string(),
                logo: logo(t),
            })
        })
        .collect();
    teams.sort_by(|a, b| a.name.cmp(&b.name));
    teams
}

/// A match seen from one team's side.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub id: String,
    pub start: DateTime<Utc>,
    /// False when the kick-off time is not fixed yet.
    pub time_confirmed: bool,
    pub competition: String,
    /// The stage, e.g. "League Phase" or "Fourth Round", when it is not the regular season.
    pub round: Option<String>,
    pub home: bool,
    pub home_team: String,
    pub away_team: String,
    pub opponent: String,
    pub opponent_id: String,
    pub opponent_logo: Option<String>,
    pub venue: Option<String>,
    pub city: Option<String>,
    /// ESPN's `pre`, `in` or `post`.
    pub state: String,
}

impl Match {
    pub fn in_progress(&self) -> bool {
        self.state == "in"
    }

    pub fn title(&self) -> String {
        format!("{} - {}", self.home_team, self.away_team)
    }

    pub fn home_away(&self) -> &'static str {
        if self.home {
            "hjemme"
        } else {
            "ude"
        }
    }
}

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    // ESPN writes "2026-10-25T17:00Z", without seconds
    DateTime::parse_from_rfc3339(s)
        .or_else(|_| DateTime::parse_from_str(&s.replace('Z', "+00:00"), "%Y-%m-%dT%H:%M%:z"))
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// A team's matches from `/{competition}/teams/{id}/schedule?fixture=true`.
pub fn parse_schedule(competition: &str, team_id: &str, v: &Value) -> Vec<Match> {
    v["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let c = &e["competitions"][0];
            let teams = c["competitors"].as_array()?;
            let us = teams.iter().find(|t| t["id"].as_str() == Some(team_id))?;
            let them = teams.iter().find(|t| t["id"].as_str() != Some(team_id))?;
            let side = |home: bool| teams.iter().find(|t| (t["homeAway"] == "home") == home).and_then(|t| t["team"]["displayName"].as_str());
            let round = e["seasonType"]["name"].as_str().filter(|r| *r != "Regular Season").map(String::from);
            Some(Match {
                id: e["id"].as_str()?.to_string(),
                start: parse_time(e["date"].as_str()?)?,
                time_confirmed: e["timeValid"].as_bool().unwrap_or(true),
                competition: competition_name(competition).to_string(),
                round,
                home: us["homeAway"] == "home",
                home_team: side(true)?.to_string(),
                away_team: side(false)?.to_string(),
                opponent: them["team"]["displayName"].as_str()?.to_string(),
                opponent_id: them["id"].as_str().unwrap_or_default().to_string(),
                opponent_logo: logo(&them["team"]),
                venue: c["venue"]["fullName"].as_str().map(String::from),
                city: c["venue"]["address"]["city"].as_str().map(String::from),
                state: c["status"]["type"]["state"].as_str().unwrap_or("pre").to_string(),
            })
        })
        .collect()
}

/// The matches not played yet, earliest first. A match that started more
/// than four hours ago is over, even if ESPN has not said so.
pub fn upcoming(mut matches: Vec<Match>, now: DateTime<Utc>) -> Vec<Match> {
    let over = now - chrono::Duration::hours(4);
    matches.retain(|m| m.state != "post" && m.start > over);
    matches.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.id.cmp(&b.id)));
    matches.dedup_by(|a, b| a.id == b.id);
    matches
}
