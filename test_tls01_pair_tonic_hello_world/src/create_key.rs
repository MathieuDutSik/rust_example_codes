use rcgen::generate_simple_self_signed;
use std::fs::File;
use std::io::Write;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    // Generate a simple self-signed certificate
    // Using localhost in the list of names passes.
    // But using "127.0.0.1" does not
    let subject_alt_names = vec!["localhost".to_string()];
    let cert = generate_simple_self_signed(subject_alt_names)?;

    // Write the certificate to a file (PEM format)
    let mut cert_file = File::create("self_signed_cert.pem")?;
    cert_file.write_all(cert.serialize_pem()?.as_bytes())?;

    // Write the private key to a file (PEM format)
    let mut key_file = File::create("private_key.pem")?;
    key_file.write_all(cert.serialize_private_key_pem().as_bytes())?;

    println!("Certificate and private key successfully written to files.");

    Ok(())
}
