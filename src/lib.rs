//! The next match for one or more football teams as an external integration
//! for Homewright.
//!
//! The fixtures come from ESPN's JSON. `espn` knows the leagues and reads the
//! answers, and this file connects them to Homewright through the
//! `thehome-plugin` SDK.

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

/// Logging to Homewright's log through stderr.
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
        Self { base: base.trim_end_matches('/').to_string(), poll, tasks: Mutex::default() }
    }

    async fn teams_in(&self, client: &reqwest::Client, leagues: &[String]) -> Result<Vec<Team>, String> {
        let mut teams = vec![];
        for l in leagues {
            let v = get(client, &format!("{}/{l}/teams", self.base)).await?.ok_or_else(|| format!("{l}: ikke fundet"))?;
            teams.extend(espn::parse_teams(l, &v));
        }
        Ok(teams)
    }

    /// A team's matches in every competition it can play in.
    async fn matches(&self, client: &reqwest::Client, team: &Team) -> Result<Vec<Match>, String> {
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
                let teams: Vec<Team> = candidates.iter().filter(|t| picked.contains(&t.key())).cloned().collect();
                if teams.is_empty() {
                    let schema = teams_schema(&candidates, &[]);
                    return Ok(Err(form_with("teams", schema, Some("Vælg mindst ét hold."), Some(json!({ "teams": "no_teams" })))));
                }
                Ok(Ok(teams))
            }
            (_, Some(v)) => {
                let leagues: Vec<String> = str_list(&v["leagues"]).into_iter().filter(|l| espn::league(l).is_some()).collect();
                if leagues.is_empty() {
                    let r = form_with(step, leagues_schema(&[]), Some("Vælg mindst én liga."), Some(json!({ "leagues": "no_leagues" })));
                    return Ok(Err(r));
                }
                let candidates = match self.teams_in(&client()?, &leagues).await {
                    Ok(t) => t,
                    Err(e) => {
                        warn!("kunne ikke hente holdene: {e}");
                        return Ok(Err(form_with(step, leagues_schema(&leagues), None, Some(json!({ "base": "cannot_connect" })))));
                    }
                };
                let selected: Vec<String> = current.iter().map(Team::key).collect();
                let mut data = flow.data.clone();
                data.insert("candidates".into(), Value::Array(candidates.iter().map(Team::to_json).collect()));
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
                "manufacturer": "ESPN",
                "model": espn::league(&team.league).map(|l| l.name).unwrap_or("Fodboldhold"),
                "entry_type": "service",
                "configuration_url": format!("https://www.espn.com/soccer/team/fixtures/_/id/{}", team.id),
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
                a.insert("time_confirmed".into(), json!(m.time_confirmed));
                a.insert("in_progress".into(), json!(m.in_progress()));
            }
            a.insert("upcoming".into(), Value::Array(list));
            if let Some(logo) = &self.team.logo {
                a.insert("entity_picture".into(), json!(logo));
            }
            a.insert("attribution".into(), json!("Kampprogram fra ESPN"));
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
