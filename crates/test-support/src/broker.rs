//! A throwaway authorization-enabled `nats-server` for identity-matrix tests.
//!
//! The broker authenticates one all-permissions administrator by user and password, so a test can
//! provision streams and durables the way Edge does, plus any number of nkey identities whose
//! permission stanzas the caller supplies. The caller proves a refusal through a missing
//! acknowledgement or an error, because a denied publish is visible to the server log only.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Why a throwaway broker could not be prepared or started.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BrokerError {
    /// The `nats-server` executable could not be started.
    #[error("the nats-server binary could not be started")]
    Spawn(#[source] std::io::Error),
    /// Broker files could not be written.
    #[error("broker files could not be written")]
    Io(#[from] std::io::Error),
    /// No port in the permitted range was free.
    #[error("no free port is available for the broker")]
    NoFreePort,
    /// The broker exited or did not accept connections in time.
    #[error("the broker did not become ready")]
    NotReady,
    /// An nkey pair could not be generated or encoded.
    #[error("an nkey pair could not be generated")]
    Key(#[from] nkeys::error::Error),
    /// The permission fragment carries no public-key placeholder.
    #[error("the permission fragment has no public-key placeholder")]
    MissingPlaceholder,
}

/// One generated user nkey pair.
#[derive(Debug)]
pub struct TestIdentity {
    public_key: String,
    seed: String,
}

impl TestIdentity {
    /// Generates a fresh user key pair.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError::Key`] when the key material cannot be encoded.
    pub fn generate() -> Result<Self, BrokerError> {
        let pair = nkeys::KeyPair::new_user();
        Ok(Self {
            public_key: pair.public_key(),
            seed: pair.seed()?,
        })
    }

    /// The public key a permission stanza names.
    #[must_use]
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    /// Writes the seed to `<directory>/<name>.nkey` readable only by the owner.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError::Io`] when the file cannot be written.
    pub fn write_seed(&self, directory: &Path, name: &str) -> Result<PathBuf, BrokerError> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;

        let path = directory.join(format!("{name}.nkey"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(self.seed.as_bytes())?;
        Ok(path)
    }

    /// Replaces the `UREPLACE_ME_...` public-key placeholder in a permission fragment with this
    /// identity's public key.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError::MissingPlaceholder`] when the fragment has no placeholder.
    pub fn substitute_into(&self, fragment: &str) -> Result<String, BrokerError> {
        const MARKER: &str = "UREPLACE_ME_";
        let start = fragment
            .find(MARKER)
            .ok_or(BrokerError::MissingPlaceholder)?;
        let length = fragment.get(start..).map_or(0, |rest| {
            rest.chars()
                .take_while(|character| {
                    character.is_ascii_uppercase()
                        || character.is_ascii_digit()
                        || *character == '_'
                })
                .count()
        });
        let end = start + length;
        Ok(format!(
            "{}{}{}",
            fragment.get(..start).unwrap_or_default(),
            self.public_key,
            fragment.get(end..).unwrap_or_default()
        ))
    }
}

/// Serializes port selection and startup, so parallel tests of one process never race for a port.
static START: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A running `nats-server` with `JetStream` and authorization enabled.
#[derive(Debug)]
pub struct AuthorizedBroker {
    child: Child,
    url: String,
    directory: PathBuf,
    admin_password: String,
}

impl AuthorizedBroker {
    /// The administrator's user name.
    pub const ADMIN_USER: &'static str = "admin";

    /// Starts a broker whose users are the administrator plus the supplied `users` entries.
    ///
    /// `users` is the text of zero or more NATS user objects, for example a permission fragment
    /// after [`TestIdentity::substitute_into`].
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError`] when the executable, port, or files are unavailable or the broker
    /// does not become ready.
    pub async fn start(users: &str) -> Result<Self, BrokerError> {
        let _startup = START.lock().await;
        let directory = std::env::temp_dir().join(format!(
            "ratatoskr-extractor-broker-{}",
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(directory.join("jetstream"))?;
        let port = free_port()?;
        let admin_password = uuid::Uuid::now_v7().simple().to_string();
        let configuration = format!(
            "port: {port}\nhost: 127.0.0.1\njetstream {{\n  store_dir: \"{store}\"\n}}\n\
             authorization {{\n  users: [\n    {{ user: \"{admin}\", password: \"{admin_password}\", \
             permissions: {{ publish: \">\", subscribe: \">\" }} }}\n{users}\n  ]\n}}\n",
            store = directory.join("jetstream").display(),
            admin = Self::ADMIN_USER,
        );
        let configuration_path = directory.join("broker.conf");
        std::fs::write(&configuration_path, configuration)?;
        let child = Command::new(server_binary())
            .arg("-c")
            .arg(&configuration_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(BrokerError::Spawn)?;
        let mut broker = Self {
            child,
            url: format!("nats://127.0.0.1:{port}"),
            directory,
            admin_password,
        };
        broker.wait_until_ready(port).await?;
        Ok(broker)
    }

    /// The client URL.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The administrator's password.
    #[must_use]
    pub fn admin_password(&self) -> &str {
        &self.admin_password
    }

    /// A private directory removed with the broker, for seed files.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    async fn wait_until_ready(&mut self, port: u16) -> Result<(), BrokerError> {
        for _ in 0..100 {
            if self.child.try_wait()?.is_some() {
                return Err(BrokerError::NotReady);
            }
            if tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .is_ok()
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err(BrokerError::NotReady)
    }
}

impl Drop for AuthorizedBroker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "test-only broker location is not process configuration"
)]
fn server_binary() -> String {
    std::env::var("NATS_SERVER_BIN").unwrap_or_else(|_| "nats-server".to_owned())
}

/// Picks a free loopback port, from `EXTRACTOR_TEST_BROKER_PORTS` (`low-high`) when set and from
/// the operating system otherwise.
#[expect(
    clippy::disallowed_methods,
    reason = "test-only broker location is not process configuration"
)]
fn free_port() -> Result<u16, BrokerError> {
    let range = std::env::var("EXTRACTOR_TEST_BROKER_PORTS")
        .ok()
        .and_then(|value| {
            let (low, high) = value.split_once('-')?;
            Some((
                low.trim().parse::<u16>().ok()?,
                high.trim().parse::<u16>().ok()?,
            ))
        });
    let free = |port: u16| std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port));
    match range {
        Some((low, high)) => (low..=high)
            .find(|port| free(*port).is_ok())
            .ok_or(BrokerError::NoFreePort),
        None => Ok(free(0)?.local_addr()?.port()),
    }
}
