//! The client against a local server standing in for doi.org, arXiv and Crossref.

use test_support::http::{Response, Server, serve, serve_paths};
use tex_cite::{Candidate, Client};

const BIBTEX: &str = "@article{Knuth_1984, title={Literate Programming}, year={1984}}";

/// A server that nothing should ask.
fn unused() -> Server {
    serve(|req| panic!("unexpected request {}", req.path))
}

fn doi_client(doi: &Server) -> Client {
    Client::with_bases(doi.url(), "http://127.0.0.1:9", "http://127.0.0.1:9")
}

fn atom(id: &str, extra: &str) -> String {
    format!(
        "<?xml version=\"1.0\"?><feed><title>ArXiv Query</title><entry>\
         <id>http://arxiv.org/abs/{id}</id><published>1999-01-04T00:00:00Z</published>\
         <title>Strings and\n  Branes</title><author><name>E. Witten</name></author>\
         {extra}</entry></feed>"
    )
}

#[test]
fn doi_asks_for_bibtex() {
    let server = serve_paths(vec![("/10.1000/xyz", Response::ok(BIBTEX))]);
    assert_eq!(doi_client(&server).doi(" 10.1000/xyz ").unwrap(), BIBTEX);
    let req = &server.requests()[0];
    assert_eq!(req.method, "GET");
    assert_eq!(
        req.header("accept"),
        Some("application/x-bibtex; charset=utf-8")
    );
    assert!(
        req.header("user-agent")
            .is_some_and(|ua| ua.starts_with("TeXoria/")),
        "{req:?}"
    );
}

#[test]
fn doi_escapes_the_path() {
    let server = serve_paths(vec![(
        "/10.1002/(SICI)1097-4571%3C1%3E3.0.CO;2-%23",
        Response::ok(BIBTEX),
    )]);
    let bib = doi_client(&server).doi("10.1002/(SICI)1097-4571<1>3.0.CO;2-#");
    assert_eq!(bib.unwrap(), BIBTEX);
}

#[test]
fn doi_follows_redirects() {
    let server = serve_paths(vec![
        ("/10.1/x", Response::redirect("/registrar/x")),
        ("/registrar/x", Response::ok(BIBTEX)),
    ]);
    assert_eq!(doi_client(&server).doi("10.1/x").unwrap(), BIBTEX);
}

#[test]
fn doi_reports_failures() {
    let server = serve_paths(vec![
        (
            "/10.1/html",
            Response::ok("<!doctype html><p>Landing page</p>"),
        ),
        ("/10.1/broken", Response::status(500)),
    ]);
    let client = doi_client(&server);
    let err = client.doi("10.1/missing").unwrap_err();
    assert_eq!(format!("{err:#}"), "No DOI 10.1/missing");
    let err = client.doi("10.1/html").unwrap_err();
    assert_eq!(format!("{err:#}"), "doi.org has no BibTeX for 10.1/html");
    let err = client.doi("10.1/broken").unwrap_err();
    assert!(
        format!("{err:#}").starts_with("looking up 10.1/broken"),
        "{err:#}"
    );
}

#[test]
fn doi_refuses_huge_answers() {
    let mut body = BIBTEX.as_bytes().to_vec();
    body.resize(3 << 20, b' ');
    let server = serve_paths(vec![("/10.1/huge", Response::ok(body))]);
    assert!(doi_client(&server).doi("10.1/huge").is_err());
}

#[test]
fn unreachable_server_is_an_error() {
    // Bound and dropped: nothing listens there any more.
    let base = test_support::http::closed_url();
    let client = Client::with_bases(&base, &base, &base);
    assert!(client.doi("10.1/x").is_err());
    assert!(client.arxiv("2301.01234").is_err());
    assert!(client.search("x").is_err());
}

#[test]
fn arxiv_builds_an_entry() {
    let arxiv = serve_paths(vec![(
        "/api/query?id_list=hep-th%2F9901001",
        Response::ok(atom("hep-th/9901001v1", "")),
    )]);
    let client = Client::with_bases(unused().url(), &format!("{}/", arxiv.url()), "x");
    let bib = client.arxiv("hep-th/9901001").unwrap();
    let e = &tex_project::bib::parse(&bib)[0];
    assert_eq!(e.key, "arxivhep-th9901001v1");
    assert_eq!(e.title.as_deref(), Some("Strings and Branes"));
    assert_eq!(e.author.as_deref(), Some("E. Witten"));
    assert_eq!(e.year.as_deref(), Some("1999"));
    assert_eq!(e.field("eprint"), Some("hep-th/9901001v1"));
}

#[test]
fn arxiv_prefers_the_published_doi() {
    let doi = serve_paths(vec![("/10.1/pub", Response::ok(BIBTEX))]);
    let arxiv = serve(|_| Response::ok(atom("2301.01234v1", "<arxiv:doi>10.1/pub</arxiv:doi>")));
    let client = Client::with_bases(doi.url(), arxiv.url(), "x");
    assert_eq!(client.arxiv("2301.01234").unwrap(), BIBTEX);
}

#[test]
fn arxiv_falls_back_when_the_doi_fails() {
    let doi = serve(|_| Response::not_found());
    let arxiv = serve(|_| Response::ok(atom("2301.01234v1", "<arxiv:doi>10.1/gone</arxiv:doi>")));
    let client = Client::with_bases(doi.url(), arxiv.url(), "x");
    let bib = client.arxiv("2301.01234").unwrap();
    assert!(bib.starts_with("@misc{arxiv230101234v1,"), "{bib}");
    assert_eq!(doi.requests()[0].path, "/10.1/gone");
}

#[test]
fn arxiv_reports_unknown_preprints() {
    // What arXiv answers for an identifier it cannot read.
    let errors = "<feed><entry><id>http://arxiv.org/api/errors#incorrect_id_format_for_1</id>\
                  <title>Error</title><summary>incorrect id format for 1</summary></entry></feed>";
    let arxiv = serve_paths(vec![
        ("/api/query?id_list=1", Response::ok(errors)),
        ("/api/query?id_list=2", Response::ok("<feed></feed>")),
        ("/api/query?id_list=3", Response::status(503)),
    ]);
    let client = Client::with_bases(unused().url(), arxiv.url(), "x");
    let err = client.arxiv("1").unwrap_err();
    assert_eq!(format!("{err:#}"), "No arXiv preprint 1");
    assert!(client.arxiv("2").is_err());
    let err = client.arxiv("3").unwrap_err();
    assert!(
        format!("{err:#}").starts_with("looking up arXiv:3"),
        "{err:#}"
    );
}

#[test]
fn search_reads_crossref() {
    let json = r#"{"status":"ok","message":{"items":[
        {"DOI":"10.1/a","title":["Attention Is All You Need"],
         "author":[{"given":"Ashish","family":"Vaswani"}],
         "issued":{"date-parts":[[2017,6]]},"container-title":["NeurIPS"]},
        {"DOI":"10.1/b","title":["No date"],"issued":{"date-parts":[[null]]}}]}}"#;
    let crossref = serve(move |_| Response::ok(json).header("Content-Type", "application/json"));
    let client = Client::with_bases("x", "x", crossref.url());
    let found = client.search("attention & Vaswani").unwrap();
    assert_eq!(
        found,
        vec![
            Candidate {
                title: "Attention Is All You Need".into(),
                authors: vec!["Ashish Vaswani".into()],
                year: Some("2017".into()),
                venue: Some("NeurIPS".into()),
                doi: "10.1/a".into(),
            },
            Candidate {
                title: "No date".into(),
                authors: vec![],
                year: None,
                venue: None,
                doi: "10.1/b".into(),
            },
        ]
    );
    let path = &crossref.requests()[0].path;
    assert!(
        path.starts_with("/works?query.bibliographic=attention+%26+Vaswani&rows=10&"),
        "{path}"
    );
}

#[test]
fn search_reports_failures() {
    let crossref = serve(|req| {
        if req.path.contains("=broken") {
            Response::status(500)
        } else if req.path.contains("=empty") {
            Response::ok(r#"{"message":{}}"#)
        } else {
            Response::ok("<html>not json</html>")
        }
    });
    let client = Client::with_bases("x", "x", crossref.url());
    let err = client.search("broken").unwrap_err();
    assert!(
        format!("{err:#}").starts_with("searching Crossref"),
        "{err:#}"
    );
    assert_eq!(client.search("empty").unwrap(), vec![]);
    let err = client.search("garbage").unwrap_err();
    assert!(
        format!("{err:#}").starts_with("reading Crossref's answer"),
        "{err:#}"
    );
}
