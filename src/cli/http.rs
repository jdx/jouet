use usage_rs::Args;

/// Send an HTTP request and print the status and only the JSON paths asked for, within a budget
///
/// Prints the status, the content type, size and time, the headers worth
/// acting on (location, retry-after, rate limits), and the body cut to
/// --budget bytes with a marker naming what was left out and the file the
/// whole body is saved in. With --select, it prints only the values at
/// those JSONPaths (RFC 9535) instead of the body, so nothing needs piping
/// through jq or Python. JSON is printed compact for agents and indented
/// for people.
///
/// --expect checks the response in hurl's style: a query (`status`, `header
/// NAME`, `jsonpath PATH`, `body`, `duration` in milliseconds), an optional
/// `count` and `not`, then a predicate (`==`, `!=`, `>`, `>=`, `<`, `<=`,
/// `contains`, `startsWith`, `endsWith`, `matches`, `exists`, `isEmpty`) and
/// its value. Without a `status` assertion, a status of 400 or more fails.
///
/// Secrets are never printed: Authorization, cookies, and headers and JSON
/// keys named like a token, password, API key or session are redacted, and so is any value of `${NAME}`, which jouet
/// expands from its own environment in the URL, headers and an inline body.
/// Give credentials that way, from the environment or through `fnox exec --
/// jouet http ...`, not written into the command; in a project that uses
/// fnox, its secrets' values are redacted too.
///
/// Exits 1 when an assertion fails, 3 when a --data or --json-body file is
/// missing, 4 when a `${NAME}` is not set, 5 when the server cannot be
/// reached, and 124 when it does not answer within --timeout.
///
/// Examples:
///   jouet http GET https://api.github.com/repos/jdx/mise --select '$.stargazers_count'
///   jouet http GET localhost:3000/health --expect 'jsonpath $.ok == true'
///   jouet http POST https://api.example.com/items --json-body '{"name":"x"}' --expect status=201
///   fnox exec -- jouet http GET https://api.example.com/me -H 'Authorization: Bearer ${API_TOKEN}'
#[derive(Args)]
pub(crate) struct HttpArgs {
    /// GET, HEAD, POST, PUT, PATCH, DELETE or OPTIONS
    pub(crate) method: String,
    /// The URL; one without a scheme is taken as http:// for localhost and
    /// https:// otherwise
    pub(crate) url: String,
    /// A request header, NAME:VALUE (repeatable)
    #[usage(short = 'H', long, value_name = "NAME:VALUE")]
    pub(crate) header: Vec<String>,
    /// Send JSON (checked to be JSON) with Content-Type: application/json;
    /// @FILE reads it from a file
    #[usage(long, value_name = "JSON")]
    pub(crate) json_body: Option<String>,
    /// Send this text as the body; @FILE reads a file and @- stdin
    #[usage(short = 'd', long, value_name = "DATA")]
    pub(crate) data: Option<String>,
    /// Print only the values at this JSONPath, such as `$.items[*].name`
    /// (repeatable)
    #[usage(short = 's', long, value_name = "JSONPATH")]
    pub(crate) select: Vec<String>,
    /// Check the response, as in `status == 200` or `jsonpath $.ok == true`,
    /// and exit 1 if it fails (repeatable)
    #[usage(short = 'e', long, value_name = "ASSERTION")]
    pub(crate) expect: Vec<String>,
    /// The most bytes of body or selected values to print (default 4000; 0
    /// is no limit)
    #[usage(long, value_name = "BYTES")]
    pub(crate) budget: Option<usize>,
    /// Print every response header, not only those worth acting on
    #[usage(short = 'i', long)]
    pub(crate) headers: bool,
    /// Do not follow redirects, so a 3xx is the answer
    #[usage(long)]
    pub(crate) no_follow: bool,
    /// Give up after DUR, such as 10s or 2m (default 30s)
    #[usage(long, value_name = "DUR")]
    pub(crate) timeout: Option<String>,
}
