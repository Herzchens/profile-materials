use std::{
    collections::HashMap,
    error::Error,
    fmt,
    sync::Mutex,
    time::{Duration, Instant},
};

use reqwest::{Client, StatusCode, header};

use super::client::{RawContributionCalendar, RawContributionDay, RawContributionWeek};

const GITHUB_ORIGIN: &str = "https://github.com";
const CURRENT_YEAR_REFRESH: Duration = Duration::from_secs(60);
const HISTORICAL_REFRESH: Duration = Duration::from_secs(6 * 60 * 60);

pub struct PublicContributionClient {
    client: Client,
    cache: Mutex<ContributionCache>,
}

#[derive(Default)]
struct ContributionCache {
    historical_current_year: Option<i32>,
    historical_days: Vec<RawContributionDay>,
    historical_refreshed_at: Option<Instant>,
    current_year: Option<i32>,
    current_days: Vec<RawContributionDay>,
    current_refreshed_at: Option<Instant>,
}

impl PublicContributionClient {
    pub fn new() -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::limited(3))
            .https_only(true)
            .timeout(Duration::from_secs(10))
            .user_agent("profile-materials/0.1 github-public-contributions")
            .build()?;
        Ok(Self {
            client,
            cache: Mutex::new(ContributionCache::default()),
        })
    }

    pub async fn fetch_calendar(
        &self,
        login: &str,
        rolling_calendar: &RawContributionCalendar,
    ) -> Result<RawContributionCalendar, PublicContributionError> {
        let current_date = latest_contribution_date(rolling_calendar)
            .ok_or(PublicContributionError::InvalidCalendar)?;
        let current_year = contribution_year(current_date)
            .ok_or(PublicContributionError::InvalidCalendar)?;

        let historical_days = self.historical_days(login, current_year).await?;
        let current_days = self.current_days(login, current_year).await?;
        Ok(build_public_calendar(
            historical_days,
            current_days,
            current_date,
        ))
    }

    async fn historical_days(
        &self,
        login: &str,
        current_year: i32,
    ) -> Result<Vec<RawContributionDay>, PublicContributionError> {
        {
            let cache = self.lock_cache();
            let fresh = cache.historical_current_year == Some(current_year)
                && cache
                    .historical_refreshed_at
                    .is_some_and(|refreshed_at| refreshed_at.elapsed() < HISTORICAL_REFRESH);
            if fresh {
                return Ok(cache.historical_days.clone());
            }
        }

        let first_year = self
            .fetch_first_contribution_year(login)
            .await?
            .unwrap_or(current_year)
            .min(current_year);
        let mut years = if current_year > 2005 {
            (first_year.max(2005)..current_year).collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if first_year < 2005 && first_year < current_year {
            years.insert(0, first_year);
        }

        let mut days = Vec::new();
        for year in years {
            days.extend(self.fetch_public_year(login, year).await?);
        }

        let mut cache = self.lock_cache();
        cache.historical_current_year = Some(current_year);
        cache.historical_days = days.clone();
        cache.historical_refreshed_at = Some(Instant::now());
        Ok(days)
    }

    async fn current_days(
        &self,
        login: &str,
        current_year: i32,
    ) -> Result<Vec<RawContributionDay>, PublicContributionError> {
        {
            let cache = self.lock_cache();
            let fresh = cache.current_year == Some(current_year)
                && cache
                    .current_refreshed_at
                    .is_some_and(|refreshed_at| refreshed_at.elapsed() < CURRENT_YEAR_REFRESH);
            if fresh {
                return Ok(cache.current_days.clone());
            }
        }

        let days = self.fetch_public_year(login, current_year).await?;
        let mut cache = self.lock_cache();
        cache.current_year = Some(current_year);
        cache.current_days = days.clone();
        cache.current_refreshed_at = Some(Instant::now());
        Ok(days)
    }

    async fn fetch_first_contribution_year(
        &self,
        login: &str,
    ) -> Result<Option<i32>, PublicContributionError> {
        let url = format!(
            "{GITHUB_ORIGIN}/{login}?action=show&controller=profiles&tab=contributions&user_id={login}"
        );
        let body = self.fetch_html(login, &url).await?;
        let years = parse_contribution_years(&body);
        Ok(years.into_iter().min())
    }

    async fn fetch_public_year(
        &self,
        login: &str,
        year: i32,
    ) -> Result<Vec<RawContributionDay>, PublicContributionError> {
        let url = format!(
            "{GITHUB_ORIGIN}/users/{login}/contributions?tab=overview&from={year:04}-12-01&to={year:04}-12-31"
        );
        let body = self.fetch_html(login, &url).await?;
        let mut days = parse_contribution_days(&body)?;
        days.retain(|day| contribution_year(&day.date) == Some(year));
        if days.is_empty() {
            return Err(PublicContributionError::Parse(format!(
                "GitHub public contribution calendar contained no days for {year}"
            )));
        }
        Ok(days)
    }

    async fn fetch_html(
        &self,
        login: &str,
        url: &str,
    ) -> Result<String, PublicContributionError> {
        let response = self
            .client
            .get(url)
            .header(header::ACCEPT, "text/html")
            .header(header::REFERER, format!("{GITHUB_ORIGIN}/{login}"))
            .header("x-requested-with", "XMLHttpRequest")
            .send()
            .await
            .map_err(PublicContributionError::Request)?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(PublicContributionError::Request)?;
        if !status.is_success() {
            return Err(PublicContributionError::Http {
                status,
                body: truncate_for_error(&body),
            });
        }
        Ok(body)
    }

    fn lock_cache(&self) -> std::sync::MutexGuard<'_, ContributionCache> {
        match self.cache.lock() {
            Ok(cache) => cache,
            Err(poisoned) => {
                tracing::warn!("GitHub public-contribution cache lock was poisoned; recovering");
                poisoned.into_inner()
            }
        }
    }
}

fn latest_contribution_date(calendar: &RawContributionCalendar) -> Option<&str> {
    calendar
        .weeks
        .iter()
        .flat_map(|week| week.contribution_days.iter())
        .map(|day| day.date.as_str())
        .max()
}

fn contribution_year(date: &str) -> Option<i32> {
    let year = date.get(..4)?.parse::<i32>().ok()?;
    (year > 0).then_some(year)
}

fn build_public_calendar(
    mut historical_days: Vec<RawContributionDay>,
    current_days: Vec<RawContributionDay>,
    current_date: &str,
) -> RawContributionCalendar {
    historical_days.extend(current_days);
    historical_days.retain(|day| day.date.as_str() <= current_date);
    historical_days.sort_unstable_by(|left, right| left.date.cmp(&right.date));
    historical_days.dedup_by(|left, right| left.date == right.date);

    if let Some(first_active) = historical_days
        .iter()
        .position(|day| day.contribution_count > 0)
    {
        historical_days.drain(..first_active);
    } else if let Some(latest) = historical_days.pop() {
        historical_days.push(latest);
    }

    let total_contributions = historical_days.iter().fold(0_u64, |total, day| {
        total.saturating_add(day.contribution_count)
    });
    let weeks = if historical_days.is_empty() {
        Vec::new()
    } else {
        vec![RawContributionWeek {
            contribution_days: historical_days,
        }]
    };
    RawContributionCalendar {
        total_contributions,
        weeks,
    }
}

fn parse_contribution_years(html: &str) -> Vec<i32> {
    let mut years = Vec::new();
    let mut cursor = 0;

    while let Some(relative) = html[cursor..].find("js-year-link") {
        let marker = cursor + relative;
        let Some(tag_start) = html[..marker].rfind('<') else {
            break;
        };
        let Some(tag_end_relative) = html[marker..].find('>') else {
            break;
        };
        let tag_end = marker + tag_end_relative;
        let tag = &html[tag_start..=tag_end];

        if let Some(href) = html_attribute(tag, "href")
            && let Some(year) = year_from_href(href)
        {
            years.push(year);
            cursor = tag_end + 1;
            continue;
        }

        if let Some(close_relative) = html[tag_end + 1..].find("</a>") {
            let inner_end = tag_end + 1 + close_relative;
            if let Some(year) = first_four_digit_year(&html[tag_end + 1..inner_end]) {
                years.push(year);
            }
            cursor = inner_end + 4;
        } else {
            cursor = tag_end + 1;
        }
    }

    years.sort_unstable();
    years.dedup();
    years
}

fn year_from_href(href: &str) -> Option<i32> {
    let marker = "from=";
    let start = href.find(marker)? + marker.len();
    let year = href.get(start..start + 4)?.parse::<i32>().ok()?;
    (year > 0).then_some(year)
}

fn first_four_digit_year(value: &str) -> Option<i32> {
    let bytes = value.as_bytes();
    for window in bytes.windows(4) {
        if window.iter().all(u8::is_ascii_digit) {
            let year = std::str::from_utf8(window).ok()?.parse::<i32>().ok()?;
            if year > 0 {
                return Some(year);
            }
        }
    }
    None
}

fn parse_contribution_days(html: &str) -> Result<Vec<RawContributionDay>, PublicContributionError> {
    let tooltips = parse_tooltip_counts(html);
    let mut days = Vec::new();
    let mut cursor = 0;

    while let Some(relative) = html[cursor..].find("ContributionCalendar-day") {
        let marker = cursor + relative;
        let Some(tag_start) = html[..marker].rfind('<') else {
            break;
        };
        let Some(tag_end_relative) = html[marker..].find('>') else {
            break;
        };
        let tag_end = marker + tag_end_relative;
        let tag = &html[tag_start..=tag_end];
        cursor = tag_end + 1;

        let Some(date) = html_attribute(tag, "data-date") else {
            continue;
        };
        let level = html_attribute(tag, "data-level")
            .and_then(|value| value.parse::<u8>().ok())
            .unwrap_or(0);
        let direct_count = html_attribute(tag, "data-count")
            .and_then(|value| value.replace(',', "").parse::<u64>().ok());
        let tooltip_count = html_attribute(tag, "id")
            .and_then(|id| tooltips.get(id).copied());
        let count = direct_count.or(tooltip_count).unwrap_or(0);

        if level > 0 && count == 0 {
            return Err(PublicContributionError::Parse(format!(
                "GitHub public contribution day {date} had activity but no parsable count"
            )));
        }
        days.push(RawContributionDay {
            contribution_count: count,
            date: date.to_owned(),
        });
    }

    days.sort_unstable_by(|left, right| left.date.cmp(&right.date));
    days.dedup_by(|left, right| left.date == right.date);
    if days.is_empty() {
        return Err(PublicContributionError::Parse(
            "GitHub public contribution calendar contained no contribution days".to_owned(),
        ));
    }
    Ok(days)
}

fn parse_tooltip_counts(html: &str) -> HashMap<String, u64> {
    let mut counts = HashMap::new();
    let mut cursor = 0;

    while let Some(relative) = html[cursor..].find("<tool-tip") {
        let start = cursor + relative;
        let Some(tag_end_relative) = html[start..].find('>') else {
            break;
        };
        let tag_end = start + tag_end_relative;
        let tag = &html[start..=tag_end];
        let Some(close_relative) = html[tag_end + 1..].find("</tool-tip>") else {
            break;
        };
        let inner_end = tag_end + 1 + close_relative;

        if let Some(target) = html_attribute(tag, "for") {
            let count = leading_count(&html[tag_end + 1..inner_end]).unwrap_or(0);
            counts.insert(target.to_owned(), count);
        }
        cursor = inner_end + "</tool-tip>".len();
    }

    counts
}

fn leading_count(value: &str) -> Option<u64> {
    let trimmed = value.trim_start();
    let number = trimmed
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == ',')
        .collect::<String>();
    if number.is_empty() {
        return None;
    }
    number.replace(',', "").parse::<u64>().ok()
}

fn html_attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let pattern = format!("{name}=");
    let mut cursor = 0;

    while let Some(relative) = tag[cursor..].find(&pattern) {
        let start = cursor + relative;
        let boundary_ok = start == 0
            || tag[..start]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_ascii_whitespace() || character == '<');
        if !boundary_ok {
            cursor = start + pattern.len();
            continue;
        }

        let value_start = start + pattern.len();
        let quote = tag.as_bytes().get(value_start).copied()?;
        if quote == b'\'' || quote == b'"' {
            let content_start = value_start + 1;
            let close = tag[content_start..].find(char::from(quote))?;
            return Some(&tag[content_start..content_start + close]);
        }

        let end = tag[value_start..]
            .find(|character: char| character.is_ascii_whitespace() || character == '>')
            .unwrap_or(tag.len() - value_start);
        return Some(&tag[value_start..value_start + end]);
    }

    None
}

fn truncate_for_error(value: &str) -> String {
    const LIMIT: usize = 256;
    let mut output = value.chars().take(LIMIT).collect::<String>();
    if value.chars().count() > LIMIT {
        output.push('…');
    }
    output
}

#[derive(Debug)]
pub enum PublicContributionError {
    Http { status: StatusCode, body: String },
    InvalidCalendar,
    Parse(String),
    Request(reqwest::Error),
}

impl fmt::Display for PublicContributionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http { status, body } => {
                write!(formatter, "GitHub public contribution HTTP error {status}: {body}")
            }
            Self::InvalidCalendar => {
                formatter.write_str("GitHub rolling contribution calendar has no current date")
            }
            Self::Parse(message) => write!(formatter, "GitHub public contribution parse error: {message}"),
            Self::Request(error) => write!(formatter, "GitHub public contribution request error: {error}"),
        }
    }
}

impl Error for PublicContributionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Request(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RawContributionDay, build_public_calendar, parse_contribution_days,
        parse_contribution_years,
    };

    #[test]
    fn parses_public_profile_year_links() {
        let html = r#"
            <a class="js-year-link" href="/Herzchens?tab=overview&from=2026-12-01&to=2026-12-31">2026</a>
            <a class="js-year-link" href="/Herzchens?tab=overview&from=2024-12-01&to=2024-12-31"><span>2024</span></a>
        "#;
        assert_eq!(parse_contribution_years(html), vec![2024, 2026]);
    }

    #[test]
    fn parses_public_calendar_tooltip_counts() {
        let html = r#"
            <td id="day-a" class="ContributionCalendar-day" data-date="2026-09-14" data-level="0"></td>
            <td id="day-b" class="ContributionCalendar-day" data-date="2026-09-15" data-level="3"></td>
            <tool-tip for="day-a">No contributions on September 14th.</tool-tip>
            <tool-tip for="day-b">14 contributions on September 15th.</tool-tip>
        "#;
        let days = parse_contribution_days(html).unwrap();
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].contribution_count, 0);
        assert_eq!(days[1].contribution_count, 14);
    }

    #[test]
    fn public_calendar_uses_exact_visible_counts_and_current_date() {
        let historical = vec![RawContributionDay {
            contribution_count: 286,
            date: "2024-04-12".to_owned(),
        }];
        let current = vec![
            RawContributionDay {
                contribution_count: 4_079,
                date: "2026-09-15".to_owned(),
            },
            RawContributionDay {
                contribution_count: 99,
                date: "2026-09-16".to_owned(),
            },
        ];
        let calendar = build_public_calendar(historical, current, "2026-09-15");
        assert_eq!(calendar.total_contributions, 4_365);
        let days = &calendar.weeks[0].contribution_days;
        assert_eq!(days.first().unwrap().date, "2024-04-12");
        assert_eq!(days.last().unwrap().date, "2026-09-15");
    }

    #[test]
    fn active_day_without_count_fails_closed() {
        let html = r#"
            <td id="day-a" class="ContributionCalendar-day" data-date="2026-09-15" data-level="3"></td>
        "#;
        assert!(parse_contribution_days(html).is_err());
    }
}
