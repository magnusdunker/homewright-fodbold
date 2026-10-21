//! The next match for one or more football teams as an external integration
//! for HomewrightHQ.
//!
//! The fixtures come from ESPN's JSON, and for the 1. division from DBU's
//! results pages. `espn` knows the leagues and reads ESPN's answers, `dbu`
//! reads DBU's pages, and this file connects them to HomewrightHQ through the
//! `thehome-plugin` SDK.

pub mod dbu;
pub mod espn;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Datelike, Local, SecondsFormat, Timelike, Utc};
use espn::{Match, Team};
use thehome_plugin::{async_trait, json, Entity, Entry, Error, Flow, FlowReply, Host, Integration, Map, Value};

pub use espn::BASE_URL;

pub const DOMAIN: &str = "fodbold";
const POLL: Duration = Duration::from_secs(3600);
/// Failed fetches in a row before a team's entities become unavailable.
const FAILURES: u32 = 3;
/// Matches listed in the attribute `upcoming`.
const UPCOMING: usize = 5;

/// Logging to HomewrightHQ's log through stderr.
#[doc(hidden)]
pub fn emit(level: &str, message: String) {
    thehome_plugin::log(level, message);
}

#[macro_export]
macro_rules! warn {
    ($($t:tt)*) => { $crate::emit("WARN", format!($($t)*)) };
}

fn leagues_schema(selected: &[String]) -> Value {
    let options: Vec<Value> = espn::LEAGUES.iter().map(|l| json!({ "const": l.slug, "title": l.name })).collect();
    json!({
        "type": "object",
        "required": ["leagues"],
        "properties": {
            "leagues": {
                "type": "array", "title": "Ligaer", "items": { "type": "string", "oneOf": options }, "default": selected,
                "description": "De ligaer, dine hold spiller i. Næste trin viser holdene."
            }
        }
    })
}

fn teams_schema(candidates: &[Team], selected: &[String]) -> Value {
    let several = candidates.iter().map(|t| &t.league).collect::<std::collections::HashSet<_>>().len() > 1;
    let options: Vec<Value> = candidates
        .iter()
        .map(|t| {
            let title = match espn::league(&t.league) {
                Some(l) if several => format!("{} ({})", t.name, l.name),
                _ => t.name.clone(),
            };
            json!({ "const": t.key(), "title": title })
        })
        .collect();
    json!({
        "type": "object",
        "required": ["teams"],
        "properties": {
            "teams": { "type": "array", "title": "Hold", "items": { "type": "string", "oneOf": options }, "default": selected }
        }
    })
}

fn str_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().filter_map(Value::as_str).map(String::from).collect(),
        Value::String(s) => s.split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect(),
        _ => vec![],
    }
}

fn team_list(v: Option<&Value>) -> Vec<Team> {
    v.and_then(Value::as_array).into_iter().flatten().filter_map(Team::from_json).collect()
}

fn title(teams: &[Team]) -> String {
    teams.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
}

fn form_with(step: &str, schema: Value, description: Option<&str>, errors: Option<Value>) -> FlowReply {
    let mut r = match errors {
        Some(e) => FlowReply::form_with_errors(step, schema, e),
        None => FlowReply::form(step, schema),
    };
    if let Some(d) = description {
        r.result["description"] = json!(d);
    }
    r
}

fn client() -> Result<reqwest::Client, Error> {
    reqwest::Client::builder()
        .user_agent(concat!("homewright-fodbold/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(Error::failed)
}

/// A page from DBU, and its address after redirects.
async fn get_html(client: &reqwest::Client, url: &str) -> Result<(String, String), String> {
    let r = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("HTTP {}", r.status()));
    }
    let at = r.url().to_string();
    Ok((at, r.text().await.map_err(|e| e.to_string())?))
}

async fn get(client: &reqwest::Client, url: &str) -> Result<Option<Value>, String> {
    let r = client.get(url).send().await.map_err(|e| e.to_string())?;
    // ESPN answers 400 for a team that is not in the competition
    if matches!(r.status().as_u16(), 400 | 404) {
        return Ok(None);
    }
    if !r.status().is_success() {
        return Err(format!("HTTP {}", r.status()));
    }
    r.json().await.map(Some).map_err(|e| e.to_string())
}

pub struct Fodbold {
    base: String,
    dbu: String,
    poll: Duration,
    tasks: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
}

impl Default for Fodbold {
    fn default() -> Self {
        Self::new(BASE_URL, POLL)
    }
}

impl Fodbold {
    /// With another address for ESPN and another interval, for tests.
    pub fn new(base: &str, poll: Duration) -> Self {
        Self { base: base.trim_end_matches('/').to_string(), dbu: dbu::BASE_URL.to_string(), poll, tasks: Mutex::default() }
    }

    /// With another address for DBU, for tests.
    pub fn with_dbu(mut self, url: &str) -> Self {
        self.dbu = url.trim_end_matches('/').to_string();
        self
    }

    /// The 1. division's teams this season, and its pool.
    async fn dbu_teams(&self, client: &reqwest::Client) -> Result<(String, Vec<Team>), String> {
        let (_, search) = get_html(client, &format!("{}{}", self.dbu, dbu::ROW_SEARCH)).await?;
        let (row, _) = dbu::league_rows(&search).into_iter().next().ok_or("DBU: 1. division ikke fundet")?;
        let (at, _) = get_html(client, &format!("{}/resultater/Raekke/{row}", self.dbu)).await?;
        let pool = dbu::pool_from_url(&at).ok_or_else(|| format!("DBU: ingen pulje for række {row}"))?;
        let (_, list) = get_html(client, &format!("{}/resultater/pulje/{pool}/holdoversigt", self.dbu)).await?;
        Ok((pool, dbu::parse_team_list(&list)))
    }

    /// The club of a 1. division team, from its page in `pool`.
    async fn dbu_club(&self, client: &reqwest::Client, team: &str, pool: &str) -> Result<String, String> {
        let (_, page) = get_html(client, &format!("{}/resultater/hold/{team}_{pool}/", self.dbu)).await?;
        dbu::club_of(&page).ok_or_else(|| format!("DBU: ingen klub for hold {team}"))
    }

    /// A 1. division team's matches in every pool its club's page lists for it.
    async fn dbu_matches(&self, client: &reqwest::Client, team: &Team) -> Result<Vec<Match>, String> {
        let club = team.club.as_deref().ok_or("DBU: holdets klub mangler")?;
        let (_, page) = get_html(client, &format!("{}/resultater/klub/{club}", self.dbu)).await?;
        let mut all = vec![];
        for (pool, name) in dbu::team_pools(&page, &team.id) {
            let (_, html) = get_html(client, &format!("{}/resultater/hold/{}_{pool}/kampprogram", self.dbu, team.id)).await?;
            all.extend(dbu::parse_fixtures(&html, &team.id, &name));
        }
        Ok(all)
    }

    /// The teams in the leagues, and the 1. division's pool when it is one of them.
    async fn teams_in(&self, client: &reqwest::Client, leagues: &[String]) -> Result<(Vec<Team>, Option<String>), String> {
        let mut teams = vec![];
        let mut pool = None;
        for l in leagues {
            if l == dbu::LEAGUE {
                let (p, t) = self.dbu_teams(client).await?;
                pool = Some(p);
                teams.extend(t);
                continue;
            }
            let v = get(client, &format!("{}/{l}/teams", self.base)).await?.ok_or_else(|| format!("{l}: ikke fundet"))?;
            teams.extend(espn::parse_teams(l, &v));
        }
        Ok((teams, pool))
    }

    /// A team's matches in every competition it can play in.
    async fn matches(&self, client: &reqwest::Client, team: &Team) -> Result<Vec<Match>, String> {
        if team.league == dbu::LEAGUE {
            return self.dbu_matches(client, team).await;
        }
        let competitions = espn::league(&team.league).map(|l| l.competitions()).unwrap_or_else(|| vec![team.league.as_str()]);
        let mut all = vec![];
        for (i, c) in competitions.into_iter().enumerate() {
            let url = format!("{}/{c}/teams/{}/schedule?fixture=true", self.base, team.id);
            match get(client, &url).await.map_err(|e| format!("{c}: {e}"))? {
                Some(v) => all.extend(espn::parse_schedule(c, &team.id, &v)),
                // the team is always in its own league
                None if i == 0 => return Err(format!("{c}: holdet {} findes ikke", team.id)),
                None => {}
            }
        }
        Ok(all)
    }

    /// The two steps shared by the config and the options flow: leagues, then teams.
    async fn pick(&self, flow: &Flow, step: &str, input: Option<Value>, current: &[Team]) -> Result<Result<Vec<Team>, FlowReply>, Error> {
        match (step, input) {
            ("teams", Some(v)) => {
                let candidates = team_list(flow.data.get("candidates"));
                let picked = str_list(&v["teams"]);
                let mut teams: Vec<Team> = candidates.iter().filter(|t| picked.contains(&t.key())).cloned().collect();
                if teams.is_empty() {
                    let schema = teams_schema(&candidates, &[]);
                    return Ok(Err(form_with("teams", schema, Some("Vælg mindst ét hold."), Some(json!({ "teams": "no_teams" })))));
                }
                // DBU's fixtures are found through the club
                let pool = flow.data.get("dbu_pool").and_then(Value::as_str).unwrap_or_default().to_string();
                let client = client()?;
                for t in teams.iter_mut().filter(|t| t.league == dbu::LEAGUE && t.club.is_none()) {
                    match self.dbu_club(&client, &t.id, &pool).await {
                        Ok(c) => t.club = Some(c),
                        Err(e) => {
                            warn!("{}: {e}", t.name);
                            let schema = teams_schema(&candidates, &picked);
                            return Ok(Err(form_with("teams", schema, None, Some(json!({ "base": "cannot_connect" })))));
                        }
                    }
                }
                Ok(Ok(teams))
            }
            (_, Some(v)) => {
                let leagues: Vec<String> = str_list(&v["leagues"]).into_iter().filter(|l| espn::league(l).is_some()).collect();
                if leagues.is_empty() {
                    let r = form_with(step, leagues_schema(&[]), Some("Vælg mindst én liga."), Some(json!({ "leagues": "no_leagues" })));
                    return Ok(Err(r));
                }
                let (candidates, dbu_pool) = match self.teams_in(&client()?, &leagues).await {
                    Ok(t) => t,
                    Err(e) => {
                        warn!("kunne ikke hente holdene: {e}");
                        return Ok(Err(form_with(step, leagues_schema(&leagues), None, Some(json!({ "base": "cannot_connect" })))));
                    }
                };
                let selected: Vec<String> = current.iter().map(Team::key).collect();
                let mut data = flow.data.clone();
                data.insert("candidates".into(), Value::Array(candidates.iter().map(Team::to_json).collect()));
                if let Some(p) = dbu_pool {
                    data.insert("dbu_pool".into(), json!(p));
                }
                Ok(Err(form_with("teams", teams_schema(&candidates, &selected), Some("Vælg de hold, du vil følge."), None).with_data(data)))
            }
            (_, None) => {
                let mut leagues: Vec<String> = vec![];
                for t in current {
                    if !leagues.contains(&t.league) {
                        leagues.push(t.league.clone());
                    }
                }
                Ok(Err(form_with(step, leagues_schema(&leagues), None, None)))
            }
        }
    }
}

#[async_trait]
impl Integration for Fodbold {
    async fn config_flow(&self, _host: &Host, flow: Flow, step: &str, input: Option<Value>) -> Result<FlowReply, Error> {
        match self.pick(&flow, step, input, &[]).await? {
            Err(form) => Ok(form),
            Ok(teams) => {
                let data = json!({ "teams": teams.iter().map(Team::to_json).collect::<Vec<_>>() });
                Ok(FlowReply::create(&title(&teams), data))
            }
        }
    }

    async fn options_flow(&self, _host: &Host, flow: Flow, step: &str, input: Option<Value>) -> Result<FlowReply, Error> {
        let current = flow.entry.as_ref().map(|e| team_list(e.get("teams"))).unwrap_or_default();
        match self.pick(&flow, step, input, &current).await? {
            Err(form) => Ok(form),
            Ok(teams) => Ok(FlowReply::create("", json!({ "teams": teams.iter().map(Team::to_json).collect::<Vec<_>>() }))),
        }
    }

    async fn setup_entry(&self, host: &Host, entry: Entry) -> Result<(), Error> {
        let entry_id = entry.entry_id.clone();
        let teams = team_list(entry.get("teams"));
        if teams.is_empty() {
            return Err(Error::failed("ingen hold"));
        }
        let client = client()?;
        // the first fetch here, so a setup without a connection is retried
        let mut first = vec![];
        for t in &teams {
            first.push(self.matches(&client, t).await);
        }
        if first.iter().all(Result::is_err) {
            if let Some(Err(e)) = first.first() {
                return Err(Error::not_ready(e));
            }
        }

        let mut followed = vec![];
        for team in teams {
            let device = json!({
                "identifiers": [[DOMAIN, format!("{entry_id}-{}", team.id)]],
                "name": team.name,
                "manufacturer": if team.league == dbu::LEAGUE { "DBU" } else { "ESPN" },
                "model": espn::league(&team.league).map(|l| l.name).unwrap_or("Fodboldhold"),
                "entry_type": "service",
                "configuration_url": match &team.club {
                    Some(club) => format!("https://www.dbu.dk/resultater/klub/{club}"),
                    None => format!("https://www.espn.com/soccer/team/fixtures/_/id/{}", team.id),
                },
            });
            let dev = host.request("device/register", json!({ "entry_id": entry_id, "device": device })).await?;
            let dev_id = dev["id"].as_str().ok_or_else(|| Error::failed("intet enheds-id"))?.to_string();
            let next = Entity::new("sensor", format!("{entry_id}-{}-next", team.id)).name("Næste kamp").device_class("timestamp").icon("mdi:soccer");
            let next = host.add_entity(&entry_id, next.device(&dev_id), false).await?;
            let opponent = Entity::new("sensor", format!("{entry_id}-{}-opponent", team.id)).name("Modstander").icon("mdi:shield-half-full");
            let opponent = host.add_entity(&entry_id, opponent.device(&dev_id), false).await?;
            followed.push(Followed { team, next, opponent, failures: 0 });
        }
        let now = Utc::now();
        for (f, r) in followed.iter_mut().zip(first) {
            match r {
                Ok(m) => f.show(host, m, now).await,
                Err(e) => {
                    warn!("{}: {e}", f.team.name);
                    f.failures = 1;
                }
            }
        }

        let (host, poll) = (host.clone(), self.poll);
        let this = Fodbold::new(&self.base, poll);
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(poll).await;
                for f in followed.iter_mut() {
                    match this.matches(&client, &f.team).await {
                        Ok(m) => {
                            f.failures = 0;
                            f.show(&host, m, Utc::now()).await;
                        }
                        Err(e) => {
                            f.failures += 1;
                            warn!("{}: {e}", f.team.name);
                            if f.failures == FAILURES {
                                f.unavailable(&host).await;
                            }
                        }
                    }
                }
            }
        });
        if let Some(old) = self.tasks.lock().unwrap().insert(entry_id, task) {
            old.abort();
        }
        Ok(())
    }

    async fn unload_entry(&self, _host: &Host, entry_id: &str) -> Result<(), Error> {
        if let Some(t) = self.tasks.lock().unwrap().remove(entry_id) {
            t.abort();
        }
        Ok(())
    }

    async fn diagnostics(&self, _host: &Host, entry: Entry) -> Value {
        json!({ "data": entry.data, "options": entry.options })
    }
}

/// A team and its two entities.
struct Followed {
    team: Team,
    next: Option<String>,
    opponent: Option<String>,
    failures: u32,
}

fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// "søndag 25. oktober kl. 18:00" in local time.
pub fn when_da(t: DateTime<Utc>) -> String {
    const DAYS: [&str; 7] = ["mandag", "tirsdag", "onsdag", "torsdag", "fredag", "lørdag", "søndag"];
    const MONTHS: [&str; 12] = ["januar", "februar", "marts", "april", "maj", "juni", "juli", "august", "september", "oktober", "november", "december"];
    let l = t.with_timezone(&Local);
    let day = DAYS[l.weekday().num_days_from_monday() as usize];
    let month = MONTHS[l.month0() as usize];
    format!("{day} {}. {month} kl. {:02}:{:02}", l.day(), l.hour(), l.minute())
}

fn match_json(m: &Match) -> Value {
    json!({
        "start": ts(m.start),
        "when": when_da(m.start),
        "match": m.title(),
        "opponent": m.opponent,
        "home_away": m.home_away(),
        "competition": m.competition,
        "round": m.round,
        "venue": m.venue,
        "tv": m.tv,
        "time_confirmed": m.time_confirmed,
    })
}

impl Followed {
    async fn show(&self, host: &Host, matches: Vec<Match>, now: DateTime<Utc>) {
        let up = espn::upcoming(matches, now);
        let next = up.first();
        let list: Vec<Value> = up.iter().take(UPCOMING).map(match_json).collect();

        if let Some(id) = &self.next {
            let mut a = Map::new();
            if let Some(m) = next {
                a.insert("match".into(), json!(m.title()));
                a.insert("when".into(), json!(when_da(m.start)));
                a.insert("opponent".into(), json!(m.opponent));
                a.insert("home".into(), json!(m.home));
                a.insert("home_away".into(), json!(m.home_away()));
                a.insert("competition".into(), json!(m.competition));
                a.insert("round".into(), json!(m.round));
                a.insert("venue".into(), json!(m.venue));
                a.insert("city".into(), json!(m.city));
                a.insert("tv".into(), json!(m.tv));
                a.insert("time_confirmed".into(), json!(m.time_confirmed));
                a.insert("in_progress".into(), json!(m.in_progress()));
            }
            a.insert("upcoming".into(), Value::Array(list));
            if let Some(logo) = &self.team.logo {
                a.insert("entity_picture".into(), json!(logo));
            }
            let source = if self.team.league == dbu::LEAGUE { "DBU" } else { "ESPN" };
            a.insert("attribution".into(), json!(format!("Kampprogram fra {source}")));
            let state = next.map(|m| ts(m.start)).unwrap_or_else(|| "unknown".into());
            if let Err(e) = host.set_state(id, state, a).await {
                warn!("kunne ikke opdatere {id}: {e}");
            }
        }
        if let Some(id) = &self.opponent {
            let mut a = Map::new();
            if let Some(m) = next {
                a.insert("home_away".into(), json!(m.home_away()));
                a.insert("start".into(), json!(ts(m.start)));
                a.insert("when".into(), json!(when_da(m.start)));
                a.insert("competition".into(), json!(m.competition));
                a.insert("venue".into(), json!(m.venue));
                if let Some(logo) = &m.opponent_logo {
                    a.insert("entity_picture".into(), json!(logo));
                }
            }
            let state = next.map(|m| m.opponent.clone()).unwrap_or_else(|| "unknown".into());
            if let Err(e) = host.set_state(id, state, a).await {
                warn!("kunne ikke opdatere {id}: {e}");
            }
        }
    }

    async fn unavailable(&self, host: &Host) {
        for id in self.next.iter().chain(&self.opponent) {
            let _ = host.set_unavailable(id).await;
        }
    }
}
