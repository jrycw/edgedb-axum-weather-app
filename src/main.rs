use axum::{
    extract::{Path, State},
    routing::get,
    Router,
};
use edgedb_errors::ConstraintViolationError;
use edgedb_protocol::value::Value;
use edgedb_tokio::{create_client, Client, Queryable};
use serde::Deserialize;
use std::time::Duration;
use tokio::{net::TcpListener, time::sleep};

fn select_city(filter: &str) -> String {
    let mut output = "
    with city := assert_single((select City filter .name = <str>$0)),
    select city { 
        name, 
        latitude, 
        longitude,
        conditions: { temperature, time }
    } "
    .to_string();
    output.push_str(filter);
    output
}

fn select_cities(filter: &str) -> String {
    let mut output = "select City { 
        name, 
        latitude, 
        longitude,
        conditions: { temperature, time }
    } "
    .to_string();
    output.push_str(filter);
    output
}

fn insert_city() -> &'static str {
    "insert City {
        name := <str>$0,
        latitude := <float64>$1,
        longitude := <float64>$2,
    };"
}

fn insert_conditions() -> &'static str {
    "insert Conditions {
        city := assert_single((select City filter .name = <str>$0)),
        temperature := <float64>$1,
        time := <str>$2 
    }"
}

fn delete_city() -> &'static str {
    "delete City filter .name = <str>$0"
}

fn select_city_names() -> &'static str {
    "select City.name order by City.name"
}

#[derive(Queryable, Debug)]
struct City {
    name: String,
    latitude: f64,
    longitude: f64,
    conditions: Option<Vec<CurrentWeather>>,
}

#[derive(Deserialize, Queryable, Debug)]
struct WeatherResult {
    current_weather: CurrentWeather,
}

#[derive(Deserialize, Queryable, Debug)]
struct CurrentWeather {
    temperature: f64,
    time: String,
}

async fn weather_for(
    latitude: f64,
    longitude: f64,
) -> Result<CurrentWeather, anyhow::Error> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?\
        latitude={latitude}&longitude={longitude}\
        &current_weather=true&timezone=CET"
    );
    let res = reqwest::get(url).await?.text().await?;
    let weather_result: WeatherResult = serde_json::from_str(&res)?;
    Ok(weather_result.current_weather)
}

struct WeatherApp {
    db: Client,
}

impl WeatherApp {
    async fn init(&self) {
        let city_data = [
            ("Andorra la Vella", 42.3, 1.3),
            ("El Serrat", 42.37, 1.33),
            ("Encamp", 42.32, 1.35),
            ("Les Escaldes", 42.3, 1.32),
            ("Sant Julià de Lòria", 42.28, 1.29),
            ("Soldeu", 42.34, 1.4),
        ];
        let query = insert_city();
        for (name, lat, long) in city_data {
            match self.db.execute(query, &(name, lat, long)).await {
                Ok(_) => println!("City {name} inserted!"),
                Err(e) => {
                    if e.is::<ConstraintViolationError>() {
                        println!("City {name} already in db");
                    } else {
                        println!("Unexpected error: {e:?}");
                    }
                }
            }
        }
    }

    async fn get_cities(&self) -> Result<Vec<City>, anyhow::Error> {
        Ok(self.db.query::<City, _>(&select_cities(""), &()).await?)
    }

    async fn update_conditions(&self) -> Result<(), anyhow::Error> {
        for City {
            name,
            latitude,
            longitude,
            ..
        } in self.get_cities().await?
        {
            let CurrentWeather { temperature, time } =
                weather_for(latitude, longitude).await?;
            match self
                .db
                .execute(insert_conditions(), &(&name, temperature, time))
                .await
            {
                Ok(()) => println!("Inserted new conditions for {}", name),
                Err(e) => {
                    if !e.is::<ConstraintViolationError>() {
                        println!("Unexpected error: {e}");
                    }
                }
            }
        }
        Ok(())
    }

    async fn run(&self) {
        sleep(Duration::from_millis(100)).await;
        loop {
            println!("Looping...");
            if let Err(e) = self.update_conditions().await {
                println!("Loop isn't working: {e}")
            }
            sleep(Duration::from_secs(60)).await;
        }
    }
}

// Axum functions
async fn menu() -> &'static str {
    "Routes:
            /conditions/<name>
            /add_city/<name>/<latitude>/<longitude>
            /remove_city/<name>
            /city_names"
}

async fn get_conditions(
    Path(city_name): Path<String>,
    State(client): State<Client>,
) -> String {
    let query = select_city("filter .name = <str>$0");
    match client
        .query_required_single::<City, _>(&query, &(&city_name,))
        .await
    {
        Ok(city) => {
            let mut conditions = format!("Conditions for {city_name}:\n\n");
            for condition in city.conditions.unwrap_or_default() {
                let (date, hour) =
                    condition.time.split_once("T").unwrap_or_default();
                conditions.push_str(&format!("{date} {hour}\t"));
                conditions.push_str(&format!("{}\n", condition.temperature));
            }
            conditions
        }
        Err(e) => format!("Couldn't find {city_name}: {e}"),
    }
}

async fn add_city(
    State(client): State<Client>,
    Path((name, lat, long)): Path<(String, f64, f64)>,
) -> String {
    // First make sure that Open-Meteo is okay with it
    let (temperature, time) = match weather_for(lat, long).await {
        Ok(c) => (c.temperature, c.time),
        Err(e) => {
            return format!("Couldn't get weather info: {e}");
        }
    };
    // Then insert the City
    if let Err(e) = client.execute(insert_city(), &(&name, lat, long)).await {
        return e.to_string();
    }
    // And finally the Conditions
    if let Err(e) = client
        .execute(insert_conditions(), &(&name, temperature, time))
        .await
    {
        return format!(
            "Inserted City {name} but couldn't insert conditions: {e}"
        );
    }
    format!("Inserted city {name}!")
}

async fn remove_city(
    Path(name): Path<String>,
    State(client): State<Client>,
) -> String {
    match client.query::<Value, _>(delete_city(), &(&name,)).await {
        Ok(v) if v.is_empty() => format!("No city {name} found to remove!"),
        Ok(_) => format!("City {name} removed!"),
        Err(e) => e.to_string(),
    }
}

async fn city_names(State(client): State<Client>) -> String {
    match client.query::<String, _>(select_city_names(), &()).await {
        Ok(cities) => format!("{cities:#?}"),
        Err(e) => e.to_string(),
    }
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let client = create_client().await?;
    let weather_app = WeatherApp { db: client.clone() };
    weather_app.init().await;
    tokio::task::spawn(async move {
        weather_app.run().await;
    });
    let app = Router::new()
        .route("/", get(menu))
        .route("/conditions/:name", get(get_conditions))
        .route("/add_city/:name/:latitude/:longitude", get(add_city))
        .route("/remove_city/:name", get(remove_city))
        .route("/city_names", get(city_names))
        .with_state(client)
        .into_make_service();
    let listener = TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
    Ok(())
}
