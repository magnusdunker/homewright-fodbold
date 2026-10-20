//! Reading ESPN's answers from 2026-10-20, and the plugin played over a pipe
//! against a fake ESPN.

mod host;

use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use homewright_fodbold::{espn, Fodbold};
use host::FakeHost;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> Value {
    let p = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn at(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

#[test]
fn teams_in_the_superliga() {
    let teams = espn::parse_teams("den.1", &fixture("den.1-teams"));
    assert_eq!(teams.len(), 12);
    let agf = teams.iter().find(|t| t.name == "AGF").unwrap();
    assert_eq!(agf.id, "7853");
    assert_eq!(agf.key(), "den.1/7853");
    assert_eq!(agf.logo.as_deref(), Some("https://a.espncdn.com/i/teamlogos/soccer/500/7853.png"));
    // by name
    assert_eq!(teams[0].name, "AC Horsens");
    assert_eq!(espn::Team::from_json(&agf.to_json()).as_ref(), Some(agf));

    let national = espn::parse_teams("uefa.nations", &fixture("uefa.nations-teams"));
    assert!(national.iter().any(|t| t.name == "Denmark" && t.id == "479"));
}

#[test]
fn next_match_across_league_and_europe() {
    let mut all = espn::parse_schedule("den.1", "909", &fixture("den.1-909"));
    let league = all.len();
    assert!(league >= 8, "{league}");
    all.extend(espn::parse_schedule("uefa.europa.conf", "909", &fixture("uefa.europa.conf-909")));
    // not in the Champions League this season
    assert!(espn::parse_schedule("uefa.champions", "909", &fixture("uefa.champions-909")).is_empty());

    let up = espn::upcoming(all, at("2026-10-20T15:00:00Z"));
    // Thursday in Belgrade before Sunday's league match
    let m = &up[0];
    assert_eq!(m.start, at("2026-10-22T16:45:00Z"));
    assert_eq!(m.competition, "Conference League");
    assert_eq!(m.round.as_deref(), Some("League Phase"));
    assert_eq!(m.opponent, "Red Star Belgrade");
    assert!(!m.home);
    assert_eq!(m.home_away(), "ude");
    assert_eq!(m.title(), "Red Star Belgrade - F.C. København");
    assert_eq!(m.venue.as_deref(), Some("Rajko Mitic Stadium"));
    assert_eq!(m.city.as_deref(), Some("Belgrade"));
    assert!(m.time_confirmed);

    let m = &up[1];
    assert_eq!(m.start, at("2026-10-26T18:00:00Z"));
    assert_eq!(m.competition, "Superligaen");
    assert_eq!(m.round, None);
    assert_eq!(m.opponent, "Odense Boldklub");
    assert!(m.home);
    assert_eq!(m.title(), "F.C. København - Odense Boldklub");

    // in order, no match twice
    assert!(up.windows(2).all(|w| w[0].start <= w[1].start && w[0].id != w[1].id));

    // the Conference League match has started: still the next one; four hours later it is over
    assert_eq!(espn::upcoming(up.clone(), at("2026-10-22T17:30:00Z"))[0].opponent, "Red Star Belgrade");
    assert_eq!(espn::upcoming(up, at("2026-10-22T21:00:00Z"))[0].opponent, "Odense Boldklub");
}

#[test]
fn every_league_asks_its_cups() {
    let pl = espn::league("eng.1").unwrap();
    assert_eq!(pl.competitions()[..3], ["eng.1", "eng.fa", "eng.league_cup"]);
    assert!(pl.competitions().contains(&"uefa.champions"));
    assert_eq!(espn::competition_name("den.1"), "Superligaen");
    assert_eq!(espn::competition_name("fifa.worldq.uefa"), "VM-kvalifikation");
    assert_eq!(espn::competition_name("xyz.9"), "xyz.9");
}

/// An ESPN event for team 909 against `them`.
fn event(id: &str, start: DateTime<Utc>, home: bool, them: (&str, &str), state: &str) -> Value {
    let us = json!({ "id": "909", "homeAway": if home { "home" } else { "away" },
                     "team": { "id": "909", "displayName": "F.C. København", "logos": [] } });
    let other = json!({ "id": them.0, "homeAway": if home { "away" } else { "home" },
                        "team": { "id": them.0, "displayName": them.1, "logos": [{ "href": format!("https://logo/{}.png", them.0) }] } });
    // ESPN writes the time without seconds
    let date = start.format("%Y-%m-%dT%H:%MZ").to_string();
    json!({
        "id": id, "date": date, "timeValid": true, "seasonType": { "name": "Regular Season" },
        "competitions": [{ "competitors": [us, other], "venue": { "fullName": "Parken", "address": { "city": "København" } },
                           "status": { "type": { "state": state } } }]
    })
}

async fn start(server: &MockServer) -> FakeHost {
    let host = FakeHost::start(Fodbold::new(&server.uri(), Duration::from_millis(300)));
    let dir = std::env::temp_dir().join(format!("fodbold-test-{}-{}", std::process::id(), rand_suffix()));
    host.call("initialize", json!({ "api_version": 1, "data_dir": dir })).await.unwrap();
    host
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

fn flow(data: Value) -> Value {
    json!({ "flow_id": "f1", "source": "user", "data": data })
}

async fn serve(server: &MockServer, p: &str, status: u16, body: Value) {
    Mock::given(method("GET")).and(path(p)).respond_with(ResponseTemplate::new(status).set_body_json(body)).mount(server).await;
}

#[tokio::test]
async fn config_flow_leagues_then_teams() {
    let server = MockServer::start().await;
    serve(&server, "/den.1/teams", 200, fixture("den.1-teams")).await;
    serve(&server, "/eng.1/teams", 200, json!({ "sports": [{ "leagues": [{ "teams": [
        { "team": { "id": "364", "displayName": "Liverpool", "logos": [] } }
    ] }] }] }))
    .await;
    let host = start(&server).await;

    let r = host.call("config_flow", json!({ "flow": flow(json!({})), "step": "user" })).await.unwrap();
    assert_eq!(r["result"]["step_id"], "user");
    let leagues = r["result"]["schema"]["properties"]["leagues"]["items"]["oneOf"].as_array().unwrap().clone();
    assert!(leagues.contains(&json!({ "const": "den.1", "title": "Superligaen" })));
    assert!(leagues.contains(&json!({ "const": "uefa.nations", "title": "Landshold" })));

    let pick = |leagues: Value| json!({ "flow": flow(json!({})), "step": "user", "input": { "leagues": leagues } });
    let r = host.call("config_flow", pick(json!([]))).await.unwrap();
    assert_eq!(r["result"]["errors"]["leagues"], "no_leagues");
    let r = host.call("config_flow", pick(json!(["den.1", "eng.1"]))).await.unwrap();
    assert_eq!(r["result"]["step_id"], "teams");
    let teams = r["result"]["schema"]["properties"]["teams"]["items"]["oneOf"].as_array().unwrap().clone();
    assert_eq!(teams.len(), 13);
    assert!(teams.contains(&json!({ "const": "den.1/7853", "title": "AGF (Superligaen)" })), "{teams:?}");
    assert!(teams.contains(&json!({ "const": "eng.1/364", "title": "Liverpool (Premier League)" })));
    let data = r["data"].clone();

    let step = |teams: Value| json!({ "flow": flow(data.clone()), "step": "teams", "input": { "teams": teams } });
    let r = host.call("config_flow", step(json!([]))).await.unwrap();
    assert_eq!(r["result"]["errors"]["teams"], "no_teams");
    let r = host.call("config_flow", step(json!(["eng.1/364", "den.1/7853"]))).await.unwrap();
    assert_eq!(r["result"]["type"], "create_entry");
    assert_eq!(r["result"]["title"], "AGF, Liverpool");
    let saved = r["result"]["data"]["teams"].as_array().unwrap();
    assert_eq!(saved[0]["id"], "7853");
    assert_eq!(saved[0]["league"], "den.1");
    assert_eq!(saved[1]["name"], "Liverpool");

    // ESPN down
    server.reset().await;
    let r = host.call("config_flow", pick(json!(["den.1"]))).await.unwrap();
    assert_eq!(r["result"]["errors"]["base"], "cannot_connect");
}

#[tokio::test]
async fn options_flow_starts_from_the_teams_followed() {
    let server = MockServer::start().await;
    serve(&server, "/den.1/teams", 200, fixture("den.1-teams")).await;
    let host = start(&server).await;
    let entry = json!({ "entry_id": "e1", "data": { "teams": [{ "id": "7853", "name": "AGF", "league": "den.1" }] } });
    let f = json!({ "flow_id": "f2", "source": "options", "data": {}, "entry": entry });

    let r = host.call("options_flow", json!({ "flow": f, "step": "options" })).await.unwrap();
    assert_eq!(r["result"]["schema"]["properties"]["leagues"]["default"], json!(["den.1"]));
    let r = host.call("options_flow", json!({ "flow": f, "step": "options", "input": { "leagues": ["den.1"] } })).await.unwrap();
    assert_eq!(r["result"]["step_id"], "teams");
    assert_eq!(r["result"]["schema"]["properties"]["teams"]["default"], json!(["den.1/7853"]));
    // one league: no league after the name
    assert!(r["result"]["schema"]["properties"]["teams"]["items"]["oneOf"].as_array().unwrap().contains(&json!({ "const": "den.1/909", "title": "F.C. København" })));

    let mut f = f.clone();
    f["data"] = r["data"].clone();
    let r = host.call("options_flow", json!({ "flow": f, "step": "teams", "input": { "teams": ["den.1/7853", "den.1/909"] } })).await.unwrap();
    assert_eq!(r["result"]["type"], "create_entry");
    assert_eq!(r["result"]["data"]["teams"].as_array().unwrap().len(), 2);
}

async fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    for _ in 0..200 {
        if let Some(v) = f() {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("ventede forgæves på {what}");
}

#[tokio::test]
async fn next_match_and_opponent() {
    let server = MockServer::start().await;
    let now = Utc::now();
    let d = ChronoDuration::days;
    let league = json!({ "events": [
        event("1", now - d(3), true, ("575", "Brøndby IF"), "post"),
        event("2", now + d(6), true, ("11550", "Odense Boldklub"), "pre"),
        event("3", now + d(13), false, ("7853", "AGF"), "pre"),
    ] });
    let europe = json!({ "events": [event("4", now + d(2), false, ("2722", "Red Star Belgrade"), "pre")] });
    serve(&server, "/den.1/teams/909/schedule", 200, league.clone()).await;
    serve(&server, "/uefa.europa.conf/teams/909/schedule", 200, europe).await;
    serve(&server, "/uefa.champions/teams/909/schedule", 400, json!({ "code": 400, "message": "Bad request for team with value: 909" })).await;

    let host = start(&server).await;
    let entry = json!({ "entry_id": "e1", "domain": "fodbold", "title": "F.C. København",
                        "data": { "teams": [{ "id": "909", "name": "F.C. København", "league": "den.1", "logo": "https://logo/909.png" }] } });
    host.call("setup_entry", json!({ "entry": entry })).await.unwrap();

    {
        let rec = host.rec.lock().unwrap();
        assert_eq!(rec.devices.len(), 1);
        assert_eq!(rec.devices[0]["name"], "F.C. København");
        assert_eq!(rec.devices[0]["model"], "Superligaen");
        assert_eq!(rec.entities["sensor.odense_n_ste_kamp"].0["device_class"], "timestamp");
    }

    let europe_start = (now + d(2)).format("%Y-%m-%dT%H:%M:00Z").to_string();
    let (state, attrs) = host.state("sensor.odense_n_ste_kamp").unwrap();
    assert_eq!(state, europe_start);
    assert_eq!(attrs["opponent"], "Red Star Belgrade");
    assert_eq!(attrs["home_away"], "ude");
    assert_eq!(attrs["competition"], "Conference League");
    assert_eq!(attrs["match"], "Red Star Belgrade - F.C. København");
    assert_eq!(attrs["entity_picture"], "https://logo/909.png");
    assert!(attrs["when"].as_str().unwrap().contains(" kl. "), "{}", attrs["when"]);
    let up: Vec<&str> = attrs["upcoming"].as_array().unwrap().iter().map(|m| m["opponent"].as_str().unwrap()).collect();
    assert_eq!(up, ["Red Star Belgrade", "Odense Boldklub", "AGF"]);
    let (state, attrs) = host.state("sensor.odense_modstander").unwrap();
    assert_eq!(state, "Red Star Belgrade");
    assert_eq!(attrs["entity_picture"], "https://logo/2722.png");
    assert_eq!(attrs["start"], europe_start);

    // the European match is played and gone from the schedule
    server.reset().await;
    serve(&server, "/den.1/teams/909/schedule", 200, league).await;
    serve(&server, "/uefa.europa.conf/teams/909/schedule", 200, json!({ "events": [] })).await;
    wait_for("næste modstander", || (host.state("sensor.odense_modstander").unwrap().0 == "Odense Boldklub").then_some(())).await;
    let (state, attrs) = host.state("sensor.odense_n_ste_kamp").unwrap();
    assert_eq!(state, (now + d(6)).format("%Y-%m-%dT%H:%M:00Z").to_string());
    assert_eq!(attrs["home_away"], "hjemme");

    // ESPN is gone: unavailable after three failures
    server.reset().await;
    wait_for("utilgængelig", || (host.state("sensor.odense_n_ste_kamp").unwrap().0 == "unavailable").then_some(())).await;
    assert_eq!(host.state("sensor.odense_modstander").unwrap().0, "unavailable");
}

#[tokio::test]
async fn no_matches_is_unknown() {
    let server = MockServer::start().await;
    serve(&server, "/den.1/teams/909/schedule", 200, json!({ "events": [] })).await;
    let host = start(&server).await;
    let entry = json!({ "entry_id": "e3", "data": { "teams": [{ "id": "909", "name": "F.C. København", "league": "den.1" }] } });
    host.call("setup_entry", json!({ "entry": entry })).await.unwrap();
    assert_eq!(host.state("sensor.odense_n_ste_kamp").unwrap().0, "unknown");
    assert_eq!(host.state("sensor.odense_modstander").unwrap().0, "unknown");
}

#[tokio::test]
async fn setup_without_espn_is_retried() {
    let host = FakeHost::start(Fodbold::new("http://127.0.0.1:1", Duration::from_secs(60)));
    let entry = json!({ "entry_id": "e2", "data": { "teams": [{ "id": "909", "name": "F.C. København", "league": "den.1" }] } });
    let e = host.call("setup_entry", json!({ "entry": entry })).await.unwrap_err();
    assert_eq!(e["data"]["kind"], "not_ready");
}

/// Against ESPN: `cargo test -- --ignored live --nocapture`.
#[tokio::test]
#[ignore]
async fn live() {
    let client = reqwest::Client::new();
    for (league, id) in [("den.1", "7853"), ("eng.1", "364"), ("uefa.nations", "479")] {
        let mut all = vec![];
        for c in espn::league(league).unwrap().competitions() {
            let url = format!("{}/{c}/teams/{id}/schedule?fixture=true", espn::BASE_URL);
            let r = client.get(url).send().await.unwrap();
            if r.status().is_success() {
                all.extend(espn::parse_schedule(c, id, &r.json().await.unwrap()));
            }
        }
        let up = espn::upcoming(all, Utc::now());
        assert!(!up.is_empty(), "{league}/{id}");
        for m in up.iter().take(3) {
            println!("{} {} {} ({})", homewright_fodbold::when_da(m.start), m.title(), m.competition, m.home_away());
        }
    }
}
