use argh::FromArgs;
use async_rustls::rustls::{self, OwnedTrustAnchor};
use async_rustls::{webpki, TlsConnector};
use smol::io::{copy, split, AsyncWriteExt};
use smol::net::TcpStream;
use smol::prelude::*;
use smol::Unblock;
use std::convert::TryFrom;
use std::fs::File;
use std::io;
use std::io::BufReader;
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::Arc;

/// Tokio Rustls client example
#[derive(FromArgs)]
struct Options {
    /// host
    #[argh(positional)]
    host: String,

    /// port
    #[argh(option, short = 'p', default = "443")]
    port: u16,

    /// domain
    #[argh(option, short = 'd')]
    domain: Option<String>,

    /// cafile
    #[argh(option, short = 'c')]
    cafile: Option<PathBuf>,
}

fn main() -> io::Result<()> {
    smol::block_on(async {
        let options: Options = argh::from_env();

        let addr = (options.host.as_str(), options.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        let domain = options.domain.unwrap_or(options.host);
        let content = format!("GET / HTTP/1.0\r\nHost: {}\r\n\r\n", domain);

        let mut root_cert_store = rustls::RootCertStore::empty();
        if let Some(cafile) = &options.cafile {
            let mut pem = BufReader::new(File::open(cafile)?);
            let certs = rustls_pemfile::certs(&mut pem)?;
            let trust_anchors = certs.iter().map(|cert| {
                let ta = webpki::TrustAnchor::try_from_cert_der(&cert[..]).unwrap();
                OwnedTrustAnchor::from_subject_spki_name_constraints(
                    ta.subject,
                    ta.spki,
                    ta.name_constraints,
                )
            });
            root_cert_store.add_server_trust_anchors(trust_anchors);
        } else {
            root_cert_store.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(
                |ta| {
                    OwnedTrustAnchor::from_subject_spki_name_constraints(
                        ta.subject,
                        ta.spki,
                        ta.name_constraints,
                    )
                },
            ));
        }

        let config = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(root_cert_store)
            .with_no_client_auth(); // i guess this was previously the default?
        let connector = TlsConnector::from(Arc::new(config));

        let stream = TcpStream::connect(&addr).await?;

        let (stdin_obj, stdout_obj) = (std::io::stdin(), std::io::stdout());
        let (mut stdin, mut stdout) = (Unblock::new(stdin_obj), Unblock::new(stdout_obj));

        let domain = rustls::ServerName::try_from(domain.as_str())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid dnsname"))?;

        let mut stream = connector.connect(domain, stream).await?;
        stream.write_all(content.as_bytes()).await?;

        let (mut reader, mut writer) = split(stream);

        copy(&mut reader, &mut stdout)
            .or(async move {
                copy(&mut stdin, &mut writer).await?;
                writer.close().await?;
                Ok(0)
            })
            .await?;

        Ok(())
    })
}
