//! DNS rebinding and mixed-answer policy tests.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use extractor_url_routing::{
    AddressBlockClass, DnsLookup, DnsLookupError, LookupFuture, ResolutionError, ValidatingResolver,
};
use tokio::sync::Mutex;

type Answer = Result<Vec<std::net::SocketAddr>, DnsLookupError>;

#[derive(Debug, Clone)]
struct QueueLookup {
    answers: Arc<Mutex<VecDeque<Answer>>>,
    calls: Arc<AtomicUsize>,
}

impl QueueLookup {
    fn new(answers: impl IntoIterator<Item = Answer>) -> Self {
        Self {
            answers: Arc::new(Mutex::new(answers.into_iter().collect())),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl DnsLookup for QueueLookup {
    fn lookup(&self, _host: String) -> LookupFuture<'_> {
        Box::pin(async move {
            // Ordering: publish every lookup before consuming its answer.
            self.calls.fetch_add(1, Ordering::Release);
            match self.answers.lock().await.pop_front() {
                Some(answer) => answer,
                None => Err(DnsLookupError),
            }
        })
    }
}

#[tokio::test]
async fn a_mixed_dns_answer_is_denied_without_connectable_addresses()
-> Result<(), Box<dyn std::error::Error>> {
    let lookup = QueueLookup::new([Ok(vec!["93.184.216.34:0".parse()?, "10.0.0.1:0".parse()?])]);
    let resolver = ValidatingResolver::new(lookup);

    let result = resolver.resolve_host("example.test").await;
    assert_eq!(
        result,
        Err(ResolutionError::Policy {
            class: AddressBlockClass::Private
        })
    );
    let report = result.map_or_else(|error| error.to_string(), |_| String::new());
    assert!(!report.contains("10.0.0.1"));
    assert!(!report.contains("93.184.216.34"));
    Ok(())
}

#[tokio::test]
async fn every_resolution_is_revalidated() -> Result<(), Box<dyn std::error::Error>> {
    let lookup = QueueLookup::new([
        Ok(vec!["93.184.216.34:0".parse()?]),
        Ok(vec!["127.0.0.1:0".parse()?]),
    ]);
    let calls = Arc::clone(&lookup.calls);
    let resolver = ValidatingResolver::new(lookup);

    assert_eq!(
        resolver.resolve_host("example.test").await?,
        ["93.184.216.34:0".parse()?]
    );
    assert_eq!(
        resolver.resolve_host("example.test").await,
        Err(ResolutionError::Policy {
            class: AddressBlockClass::Loopback
        })
    );
    // Ordering: observe both published lookup calls.
    assert_eq!(calls.load(Ordering::Acquire), 2);
    Ok(())
}
