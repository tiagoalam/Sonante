use serde::{Deserialize, Serialize};
use std::time::Duration;

const ENDPOINT: &str = "https://musicbrainz.org/ws/2/release/";
const LIMIT: usize = 10;
const MAX_RESPONSE_BYTES: usize = 512 * 1024;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SearchError {
    InvalidQuery,
    ServiceUnavailable,
    RateLimited,
    Timeout,
    HttpError,
    InvalidResponse,
    ResponseTooLarge,
    InvalidMbid,
    ReleaseNotFound,
    IdentityMismatch,
}

#[derive(Deserialize)]
pub struct SearchRequest {
    pub title: String,
    pub artist: String,
}

#[derive(Debug, Serialize)]
pub struct ReleaseDetails {
    id: String,
    title: Option<String>,
    artist_credit: Option<String>,
    edition_date: Option<String>,
    release_group_id: Option<String>,
    first_release_date: Option<String>,
}

#[derive(Deserialize)]
struct ReleaseGroup {
    id: String,
    #[serde(rename = "first-release-date")]
    first_release_date: Option<String>,
}

#[derive(Deserialize)]
struct DetailResponse {
    id: String,
    title: Option<String>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<Credit>,
    date: Option<String>,
    #[serde(rename = "release-group")]
    release_group: Option<ReleaseGroup>,
}

fn valid_mbid(id: &str) -> bool {
    id.len() == 36
        && id.chars().enumerate().all(|(index, ch)| {
            if [8, 13, 18, 23].contains(&index) {
                ch == '-'
            } else {
                ch.is_ascii_hexdigit()
            }
        })
}

fn credit_text(credits: Vec<Credit>) -> Option<String> {
    if credits.is_empty() {
        return None;
    }
    credits
        .into_iter()
        .map(|credit| {
            let name = credit
                .name
                .or_else(|| credit.artist.and_then(|artist| artist.name))?;
            Some(format!("{}{}", name, credit.joinphrase.unwrap_or_default()))
        })
        .collect::<Option<Vec<_>>>()
        .map(|names| names.concat())
}

fn details_url(mbid: &str) -> Result<reqwest::Url, SearchError> {
    if !valid_mbid(mbid) {
        return Err(SearchError::InvalidMbid);
    }
    let mut url = reqwest::Url::parse(&format!("{ENDPOINT}{}", mbid.to_ascii_lowercase()))
        .map_err(|_| SearchError::InvalidMbid)?;
    url.query_pairs_mut()
        .append_pair("fmt", "json")
        .append_pair("inc", "artist-credits+release-groups");
    Ok(url)
}

fn parse_details(bytes: &[u8], mbid: &str) -> Result<ReleaseDetails, SearchError> {
    let details: DetailResponse =
        serde_json::from_slice(bytes).map_err(|_| SearchError::InvalidResponse)?;
    if !valid_mbid(&details.id)
        || details
            .release_group
            .as_ref()
            .is_some_and(|group| !valid_mbid(&group.id))
    {
        return Err(SearchError::InvalidResponse);
    }
    if !details.id.eq_ignore_ascii_case(mbid) {
        return Err(SearchError::IdentityMismatch);
    }
    Ok(ReleaseDetails {
        id: details.id,
        title: details.title,
        artist_credit: credit_text(details.artist_credit),
        edition_date: details.date,
        release_group_id: details.release_group.as_ref().map(|group| group.id.clone()),
        first_release_date: details
            .release_group
            .and_then(|group| group.first_release_date),
    })
}

#[derive(Debug, Serialize)]
pub struct Label {
    name: Option<String>,
    catalog_number: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Candidate {
    id: String,
    title: Option<String>,
    artist_credit: Option<String>,
    date: Option<String>,
    country: Option<String>,
    formats: Vec<Option<String>>,
    labels: Vec<Label>,
    score: Option<u8>,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    candidates: Vec<Candidate>,
    total: u64,
    limited: bool,
}

#[derive(Deserialize)]
struct Response {
    count: u64,
    offset: u64,
    releases: Vec<Release>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Score {
    Number(u8),
    Text(String),
}

#[derive(Deserialize)]
struct Named {
    name: Option<String>,
}

#[derive(Deserialize)]
struct Credit {
    name: Option<String>,
    artist: Option<Named>,
    joinphrase: Option<String>,
}

#[derive(Deserialize)]
struct Medium {
    format: Option<String>,
}

#[derive(Deserialize)]
struct LabelInfo {
    label: Option<Named>,
    #[serde(rename = "catalog-number")]
    catalog_number: Option<String>,
}

#[derive(Deserialize)]
struct Release {
    id: String,
    title: Option<String>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<Credit>,
    date: Option<String>,
    country: Option<String>,
    #[serde(default)]
    media: Vec<Medium>,
    #[serde(rename = "label-info", default)]
    labels: Vec<LabelInfo>,
    score: Option<Score>,
}

fn escape_lucene(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        if "+-&|!(){}[]^\"~*?:\\/".contains(ch) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn search_url(request: &SearchRequest) -> Result<reqwest::Url, SearchError> {
    if [&request.title, &request.artist]
        .iter()
        .any(|value| value.chars().count() > 256 || value.chars().any(char::is_control))
        || request.title.trim().is_empty()
    {
        return Err(SearchError::InvalidQuery);
    }
    let mut query = format!("release:\"{}\"", escape_lucene(request.title.trim()));
    if !request.artist.trim().is_empty() {
        query.push_str(&format!(
            " AND artist:\"{}\"",
            escape_lucene(request.artist.trim())
        ));
    }
    let mut url = reqwest::Url::parse(ENDPOINT).map_err(|_| SearchError::ServiceUnavailable)?;
    url.query_pairs_mut()
        .append_pair("query", &query)
        .append_pair("fmt", "json")
        .append_pair("limit", "10");
    Ok(url)
}

fn parse_response(bytes: &[u8]) -> Result<SearchResult, SearchError> {
    let response: Response =
        serde_json::from_slice(bytes).map_err(|_| SearchError::InvalidResponse)?;
    if response.offset != 0 || response.count < response.releases.len() as u64 {
        return Err(SearchError::InvalidResponse);
    }
    let limited = response.count > response.releases.len().min(LIMIT) as u64;
    let mut candidates = Vec::new();
    for release in response.releases.into_iter().take(LIMIT) {
        if !valid_mbid(&release.id) {
            return Err(SearchError::InvalidResponse);
        }
        let score = match release.score {
            Some(Score::Number(value)) => Some(value),
            Some(Score::Text(value)) => Some(
                value
                    .parse::<u8>()
                    .map_err(|_| SearchError::InvalidResponse)?,
            ),
            None => None,
        };
        if score.is_some_and(|score| score > 100) {
            return Err(SearchError::InvalidResponse);
        }
        let artist_credit = credit_text(release.artist_credit);
        candidates.push(Candidate {
            id: release.id,
            title: release.title,
            artist_credit,
            date: release.date,
            country: release.country,
            formats: release
                .media
                .into_iter()
                .map(|medium| medium.format)
                .collect(),
            labels: release
                .labels
                .into_iter()
                .map(|info| Label {
                    name: info.label.and_then(|label| label.name),
                    catalog_number: info.catalog_number,
                })
                .collect(),
            score,
        });
    }
    if candidates.is_empty() && response.count != 0 {
        return Err(SearchError::InvalidResponse);
    }
    Ok(SearchResult {
        candidates,
        total: response.count,
        limited,
    })
}

fn transport_error(error: reqwest::Error) -> SearchError {
    if error.is_timeout() {
        SearchError::Timeout
    } else {
        SearchError::ServiceUnavailable
    }
}

// A response seam keeps HTTP status/body handling testable without sockets.
trait HttpResponse: Send {
    fn status(&self) -> u16;
    fn content_length(&self) -> Option<u64>;
    fn next_chunk(
        &mut self,
    ) -> impl std::future::Future<Output = Result<Option<Vec<u8>>, SearchError>> + Send;
}

impl HttpResponse for reqwest::Response {
    fn status(&self) -> u16 {
        reqwest::Response::status(self).as_u16()
    }
    fn content_length(&self) -> Option<u64> {
        reqwest::Response::content_length(self)
    }
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, SearchError> {
        self.chunk()
            .await
            .map(|chunk| chunk.map(|bytes| bytes.to_vec()))
            .map_err(transport_error)
    }
}

async fn read_response_bytes(
    mut response: impl HttpResponse,
    lookup: bool,
) -> Result<Vec<u8>, SearchError> {
    match response.status() {
        200 => {}
        429 => return Err(SearchError::RateLimited),
        404 if lookup => return Err(SearchError::ReleaseNotFound),
        500..=599 => return Err(SearchError::ServiceUnavailable),
        _ => return Err(SearchError::HttpError),
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(SearchError::ResponseTooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.next_chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(SearchError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn read_response(response: impl HttpResponse) -> Result<SearchResult, SearchError> {
    parse_response(&read_response_bytes(response, false).await?)
}

async fn read_details(
    response: impl HttpResponse,
    mbid: &str,
) -> Result<ReleaseDetails, SearchError> {
    parse_details(&read_response_bytes(response, true).await?, mbid)
}

async fn fetch(client: &reqwest::Client, url: reqwest::Url) -> Result<SearchResult, SearchError> {
    let response = client.get(url).send().await.map_err(transport_error)?;
    read_response(response).await
}

async fn limited_client() -> Result<reqwest::Client, SearchError> {
    let client = reqwest::Client::builder()
        .user_agent(crate::online_artwork::USER_AGENT)
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| SearchError::ServiceUnavailable)?;
    tauri::async_runtime::spawn_blocking(crate::online_artwork::wait_for_musicbrainz)
        .await
        .map_err(|_| SearchError::ServiceUnavailable)?
        .map_err(|_| SearchError::ServiceUnavailable)?;
    Ok(client)
}

pub async fn search(request: SearchRequest) -> Result<SearchResult, SearchError> {
    let url = search_url(&request)?;
    let client = limited_client().await?;
    fetch(&client, url).await
}

pub async fn lookup(mbid: String) -> Result<ReleaseDetails, SearchError> {
    let url = details_url(&mbid)?;
    let client = limited_client().await?;
    let response = client.get(url).send().await.map_err(transport_error)?;
    read_details(response, &mbid).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    const ID: &str = "62d1c4ef-fc00-37af-8df7-485f6a31fcc4";
    fn fixture() -> Vec<u8> {
        format!(r#"{{"created":"2025-01-01T00:00:00Z","count":23,"offset":0,"releases":[{{
          "id":"{ID}","score":"97","title":"An edition","date":"2001-03-04","country":"BR",
          "artist-credit":[{{"name":"Credited A","artist":{{"name":"Canonical A"}},"joinphrase":" feat. "}},
             {{"name":"B","joinphrase":" & "}},{{"artist":{{"name":"C"}}}}],
          "media":[{{"format":"CD"}},{{"format":"Digital Media"}},{{}}],
          "label-info":[{{"label":{{"name":"Label"}},"catalog-number":"ABC-01"}},{{"catalog-number":"XYZ"}}]
        }}]}}"#).into_bytes()
    }

    #[test]
    fn parses_release_editions_and_preserves_credit_names_joinphrases_and_missing_fields() {
        let result = parse_response(&fixture()).unwrap();
        assert_eq!(result.total, 23);
        assert!(result.limited);
        let candidate = &result.candidates[0];
        assert_eq!(
            candidate.artist_credit.as_deref(),
            Some("Credited A feat. B & C")
        );
        assert_eq!(candidate.date.as_deref(), Some("2001-03-04"));
        assert_eq!(candidate.country.as_deref(), Some("BR"));
        assert_eq!(
            candidate.formats,
            vec![Some("CD".into()), Some("Digital Media".into()), None]
        );
        assert_eq!(
            candidate.labels[0].catalog_number.as_deref(),
            Some("ABC-01")
        );
        assert!(candidate.labels[1].name.is_none());
        assert_eq!(candidate.score, Some(97));
        let incomplete = parse_response(
            format!(r#"{{"count":1,"offset":0,"releases":[{{"id":"{ID}","score":0}}]}}"#)
                .as_bytes(),
        )
        .unwrap();
        let candidate = &incomplete.candidates[0];
        assert!(
            candidate.title.is_none()
                && candidate.artist_credit.is_none()
                && candidate.date.is_none()
        );
        assert!(
            candidate.country.is_none()
                && candidate.formats.is_empty()
                && candidate.labels.is_empty()
        );
        assert_eq!(candidate.score, Some(0));
        assert!(!incomplete.limited);
    }

    #[test]
    fn empty_is_distinct_from_invalid_and_results_never_exceed_ten() {
        assert!(parse_response(br#"{"count":0,"offset":0,"releases":[]}"#)
            .unwrap()
            .candidates
            .is_empty());
        let releases = (0..12)
            .map(|_| format!(r#"{{"id":"{ID}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let result = parse_response(
            format!(r#"{{"count":12,"offset":0,"releases":[{releases}]}}"#).as_bytes(),
        )
        .unwrap();
        assert_eq!(result.candidates.len(), 10);
        assert!(result.limited);
        for invalid in [
            "<html>unavailable</html>",
            "{}",
            r#"{"count":2,"offset":0,"releases":[]}"#,
            r#"{"count":0,"offset":1,"releases":[]}"#,
            r#"{"count":1,"offset":0,"releases":[{"id":"bad"}]}"#,
        ] {
            assert_eq!(
                parse_response(invalid.as_bytes()).unwrap_err(),
                SearchError::InvalidResponse
            );
        }
    }

    #[test]
    fn lucene_literals_and_url_are_escaped_without_inference_or_endpoint_injection() {
        let title = r#"A "+-&|!(){}[]^~*?:\/ AND artist:evil #&fmt=xml"#;
        let url = search_url(&SearchRequest {
            title: title.into(),
            artist: " AC/DC ".into(),
        })
        .unwrap();
        assert_eq!(url.host_str(), Some("musicbrainz.org"));
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.path(), "/ws/2/release/");
        assert!(url.fragment().is_none());
        let pairs = url.query_pairs().collect::<Vec<_>>();
        assert_eq!(pairs.len(), 3);
        assert_eq!(
            pairs[0].1,
            format!(
                "release:\"{}\" AND artist:\"AC\\/DC\"",
                escape_lucene(title)
            )
        );
        assert_eq!(
            escape_lucene(r#""\+-&|!(){}[]^~*?:/"#),
            r#"\"\\\+\-\&\|\!\(\)\{\}\[\]\^\~\*\?\:\/"#
        );
        let title_only = search_url(&SearchRequest {
            title: "Álbum".into(),
            artist: "".into(),
        })
        .unwrap();
        assert_eq!(
            title_only.query_pairs().next().unwrap().1,
            "release:\"Álbum\""
        );
        for title in ["".into(), " ".into(), "A\nB".into(), "x".repeat(257)] {
            assert_eq!(
                search_url(&SearchRequest {
                    title,
                    artist: "".into()
                })
                .unwrap_err(),
                SearchError::InvalidQuery
            );
        }
    }

    struct FakeHttpResponse {
        status: u16,
        length: Option<u64>,
        chunks: VecDeque<Result<Vec<u8>, SearchError>>,
    }
    impl HttpResponse for FakeHttpResponse {
        fn status(&self) -> u16 {
            self.status
        }
        fn content_length(&self) -> Option<u64> {
            self.length
        }
        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, SearchError> {
            self.chunks.pop_front().transpose()
        }
    }
    fn http(
        status: u16,
        length: Option<u64>,
        chunks: Vec<Result<Vec<u8>, SearchError>>,
    ) -> FakeHttpResponse {
        FakeHttpResponse {
            status,
            length,
            chunks: chunks.into(),
        }
    }

    #[test]
    fn simulated_http_covers_success_unavailability_limits_timeout_and_invalid_responses() {
        let body = fixture();
        let split = body.len() / 2;
        let response = http(
            200,
            None,
            vec![Ok(body[..split].to_vec()), Ok(body[split..].to_vec())],
        );
        assert_eq!(
            tauri::async_runtime::block_on(read_response(response))
                .unwrap()
                .candidates
                .len(),
            1
        );
        for (status, length, chunks, error) in [
            (429, None, vec![], SearchError::RateLimited),
            (503, None, vec![], SearchError::ServiceUnavailable),
            (400, None, vec![], SearchError::HttpError),
            (302, None, vec![], SearchError::HttpError),
            (
                200,
                None,
                vec![Ok(b"invalid".to_vec())],
                SearchError::InvalidResponse,
            ),
            (
                200,
                None,
                vec![Err(SearchError::Timeout)],
                SearchError::Timeout,
            ),
            (
                200,
                None,
                vec![Err(SearchError::ServiceUnavailable)],
                SearchError::ServiceUnavailable,
            ),
            (
                200,
                Some(MAX_RESPONSE_BYTES as u64 + 1),
                vec![],
                SearchError::ResponseTooLarge,
            ),
            (
                200,
                None,
                vec![Ok(vec![b' '; MAX_RESPONSE_BYTES]), Ok(vec![b' '])],
                SearchError::ResponseTooLarge,
            ),
        ] {
            assert_eq!(
                tauri::async_runtime::block_on(read_response(http(status, length, chunks)))
                    .unwrap_err(),
                error
            );
        }
        let error = reqwest::Client::new().get("not a URL").build().unwrap_err();
        assert_eq!(transport_error(error), SearchError::ServiceUnavailable);
    }
    #[test]
    fn details_url_validates_mbid_and_requests_only_credits_and_group() {
        let url = details_url(&ID.to_uppercase()).unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("musicbrainz.org"));
        assert_eq!(url.path(), format!("/ws/2/release/{ID}"));
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![
                ("fmt".into(), "json".into()),
                ("inc".into(), "artist-credits+release-groups".into())
            ]
        );
        assert!(url.fragment().is_none());
        for bad in [
            "",
            "../release-group",
            "https://example.org",
            "62d1c4ef-fc00-37af-8df7-485f6a31fcc4?inc=recordings",
            "62d1c4ef-fc00-37af-8df7-485f6a31fccZ",
        ] {
            assert_eq!(details_url(bad).unwrap_err(), SearchError::InvalidMbid);
        }
    }

    #[test]
    fn details_preserve_multiple_credits_and_distinct_dates_without_inference() {
        let bytes = format!(
            r#"{{"id":"{ID}","title":"Edition","date":"2004-03-01",
          "artist-credit":[{{"name":"Credited A","artist":{{"name":"Canonical A"}},"joinphrase":" feat. "}},
            {{"name":"B","joinphrase":" & "}},{{"artist":{{"name":"C"}}}}],
          "release-group":{{"id":"12345678-abcd-1234-abcd-123456789abc","first-release-date":"1999"}}}}"#
        );
        let details = parse_details(bytes.as_bytes(), ID).unwrap();
        assert_eq!(
            details.artist_credit.as_deref(),
            Some("Credited A feat. B & C")
        );
        assert_eq!(details.edition_date.as_deref(), Some("2004-03-01"));
        assert_eq!(details.first_release_date.as_deref(), Some("1999"));
        let partial = parse_details(
            format!(r#"{{"id":"{ID}","title":"","date":"not a date","artist-credit":[{{}}]}}"#)
                .as_bytes(),
            ID,
        )
        .unwrap();
        assert_eq!(partial.title.as_deref(), Some(""));
        assert!(
            partial.artist_credit.is_none()
                && partial.release_group_id.is_none()
                && partial.first_release_date.is_none()
        );
        assert_eq!(partial.edition_date.as_deref(), Some("not a date"));
        let group_only = parse_details(
            format!(
                r#"{{"id":"{ID}","release-group":{{"id":"{ID}","first-release-date":"1980"}}}}"#
            )
            .as_bytes(),
            ID,
        )
        .unwrap();
        assert!(group_only.edition_date.is_none());
        assert_eq!(group_only.first_release_date.as_deref(), Some("1980"));
        let missing = parse_details(format!(r#"{{"id":"{ID}"}}"#).as_bytes(), ID).unwrap();
        assert!(
            missing.title.is_none()
                && missing.artist_credit.is_none()
                && missing.edition_date.is_none()
        );
    }

    #[test]
    fn details_identity_invalid_response_and_http_failures_are_not_empty_success() {
        assert_eq!(
            parse_details(
                format!(r#"{{"id":"{ID}"}}"#).as_bytes(),
                "12345678-abcd-1234-abcd-123456789abc"
            )
            .unwrap_err(),
            SearchError::IdentityMismatch
        );
        for body in ["{}", "invalid", r#"{"id":"bad"}"#] {
            assert_eq!(
                parse_details(body.as_bytes(), ID).unwrap_err(),
                SearchError::InvalidResponse
            );
        }
        let good = format!(r#"{{"id":"{ID}"}}"#).into_bytes();
        assert_eq!(
            tauri::async_runtime::block_on(read_details(http(200, None, vec![Ok(good)]), ID))
                .unwrap()
                .id,
            ID
        );
        for (status, length, chunks, expected) in [
            (404, None, vec![], SearchError::ReleaseNotFound),
            (429, None, vec![], SearchError::RateLimited),
            (503, None, vec![], SearchError::ServiceUnavailable),
            (403, None, vec![], SearchError::HttpError),
            (302, None, vec![], SearchError::HttpError),
            (
                200,
                None,
                vec![Err(SearchError::Timeout)],
                SearchError::Timeout,
            ),
            (
                200,
                None,
                vec![Err(SearchError::ServiceUnavailable)],
                SearchError::ServiceUnavailable,
            ),
            (
                200,
                None,
                vec![Ok(b"invalid".to_vec())],
                SearchError::InvalidResponse,
            ),
            (
                200,
                Some(MAX_RESPONSE_BYTES as u64 + 1),
                vec![],
                SearchError::ResponseTooLarge,
            ),
            (
                200,
                None,
                vec![Ok(vec![b' '; MAX_RESPONSE_BYTES]), Ok(vec![b' '])],
                SearchError::ResponseTooLarge,
            ),
        ] {
            assert_eq!(
                tauri::async_runtime::block_on(read_details(http(status, length, chunks), ID))
                    .unwrap_err(),
                expected
            );
        }
        assert_eq!(
            tauri::async_runtime::block_on(read_response(http(404, None, vec![]))).unwrap_err(),
            SearchError::HttpError
        );
        assert_eq!(
            serde_json::to_string(&SearchError::ReleaseNotFound).unwrap(),
            "\"release_not_found\""
        );
    }
}
