//! SSRF destination-address policy tests.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use extractor_url_routing::{AddressBlockClass, validate_address};

#[test]
fn every_non_global_address_class_is_denied() -> Result<(), Box<dyn std::error::Error>> {
    let denied = [
        (
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            AddressBlockClass::Unspecified,
        ),
        (IpAddr::V4(Ipv4Addr::LOCALHOST), AddressBlockClass::Loopback),
        ("10.0.0.1".parse()?, AddressBlockClass::Private),
        ("172.31.0.1".parse()?, AddressBlockClass::Private),
        ("192.168.1.1".parse()?, AddressBlockClass::Private),
        ("100.64.0.1".parse()?, AddressBlockClass::Shared),
        ("169.254.1.1".parse()?, AddressBlockClass::LinkLocal),
        ("169.254.169.254".parse()?, AddressBlockClass::Metadata),
        ("192.0.2.1".parse()?, AddressBlockClass::Documentation),
        ("198.51.100.1".parse()?, AddressBlockClass::Documentation),
        ("203.0.113.1".parse()?, AddressBlockClass::Documentation),
        ("198.18.0.1".parse()?, AddressBlockClass::Benchmarking),
        ("224.0.0.1".parse()?, AddressBlockClass::Multicast),
        ("240.0.0.1".parse()?, AddressBlockClass::Reserved),
        (
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            AddressBlockClass::Unspecified,
        ),
        (IpAddr::V6(Ipv6Addr::LOCALHOST), AddressBlockClass::Loopback),
        ("fc00::1".parse()?, AddressBlockClass::Private),
        ("fe80::1".parse()?, AddressBlockClass::LinkLocal),
        ("ff02::1".parse()?, AddressBlockClass::Multicast),
        ("2001:db8::1".parse()?, AddressBlockClass::Documentation),
        ("2001:2::1".parse()?, AddressBlockClass::Benchmarking),
        ("64:ff9b::7f00:1".parse()?, AddressBlockClass::Reserved),
        ("::ffff:127.0.0.1".parse()?, AddressBlockClass::Loopback),
    ];

    for (address, class) in denied {
        let result = validate_address(address);
        assert!(result.is_err(), "accepted prohibited address {address}");
        if let Err(error) = result {
            assert_eq!(error.class, class);
            assert!(!error.to_string().contains(&address.to_string()));
        }
    }
    for public in [
        "1.1.1.1".parse()?,
        "8.8.8.8".parse()?,
        "2606:4700:4700::1111".parse()?,
    ] {
        assert!(
            validate_address(public).is_ok(),
            "blocked public control {public}"
        );
    }
    Ok(())
}
