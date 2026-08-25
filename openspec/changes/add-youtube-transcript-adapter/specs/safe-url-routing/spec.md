## MODIFIED Requirements

### Requirement: Known source ownership is classified before generic web
The extractor SHALL classify exact hosts and their documented subdomains into delegated provider routes, public source-adapter routes, direct PDF candidates, or generic web. A lookalike suffix SHALL remain generic web. The documented YouTube route hosts are `youtube.com` and its subdomains, `youtu.be`, and `youtube-nocookie.com` and its subdomains.

#### Scenario: A provider lookalike is not delegated
- **WHEN** the host is `github.com.example.test` rather than `github.com` or its documented subdomain
- **THEN** classification returns generic web and not the GitHub-owned route

#### Scenario: A nocookie embed classifies as YouTube

- **WHEN** the host is `www.youtube-nocookie.com` with an `/embed/` path
- **THEN** classification returns the YouTube route and not generic web

#### Scenario: A YouTube lookalike stays generic web

- **WHEN** the host is `youtube-nocookie.com.example.test`
- **THEN** classification returns generic web
