use std::{
	fs,
	io::{self, stdout, Cursor, Read, Write},
	path::PathBuf,
	str::FromStr,
};

use age::Recipient;
use anyhow::{anyhow, bail, ensure, Context, Result};
use clap::Parser;
use ed25519_dalek::SigningKey;
use fleet_shared::SecretData;
use rand::{
	distributions::{Alphanumeric, DistString, Distribution, Uniform},
	rngs::OsRng,
	thread_rng, Rng,
};

fn write_output(out: &str, data: impl AsRef<[u8]>, stdout_marker: &mut bool) -> Result<()> {
	let data = data.as_ref();
	if out == "-" {
		let mut stdout = stdout();
		if *stdout_marker {
			stdout.write_all(&[b'\n'])?;
		}
		*stdout_marker = true;
		stdout.write_all(data)?;
	} else {
		fs::write(out, data)?;
	};
	Ok(())
}

#[derive(Parser)]
enum Generate {
	/// Generate public, private keys without wrapping, in standard ed25519 schema
	/// (64 bytes private (due to merge with private), 32 bytes public)
	Ed25519 {
		public: String,
		private: String,
		/// Private key should be just the private key (32 bytes), not standard private+public.
		#[arg(long)]
		no_embed_public: bool,
	},
	Password {
		output: String,
		size: usize,
		#[arg(long, short = 'n')]
		no_symbols: bool,
	},
}

#[derive(Parser)]
enum Opts {
	/// Encode public part from stdin.
	Public {
		#[arg(long)]
		allow_empty: bool,
	},
	/// Encrypt private part from stdin.
	Private {
		#[arg(long)]
		allow_empty: bool,
		#[arg(short = 'r')]
		recipient: Vec<String>,
	},
	/// Generate keys in well-known schemas.
	///
	/// Note that this command is only intended to be used in fleet secret generator,
	/// otherwise you should ensure noone is able to read generated files, they don't have any mode set by default.
	#[command(subcommand)]
	Generate(Generate),
	// Generate {
	// 	kind: GenerateKind,
	// 	/// Different generators generate different number of files, you need to specify number of outputs corresponding to the generator.
	// 	#[arg(short = 'o')]
	// 	outputs: Vec<String>,
	// },
}

fn parse_stdin() -> Result<Option<Vec<u8>>> {
	let mut input = vec![];
	io::stdin().read_to_end(&mut input)?;
	if input.is_empty() {
		Ok(None)
	} else {
		Ok(Some(input))
	}
}
pub fn encrypt_secret_data(
	recipients: impl IntoIterator<Item = impl Recipient + Send + 'static>,
	data: Vec<u8>,
) -> Option<SecretData> {
	let mut encrypted = vec![];
	let recipients = recipients
		.into_iter()
		.map(|v| Box::new(v) as Box<dyn Recipient + Send>)
		.collect::<Vec<_>>();
	let mut encryptor = age::Encryptor::with_recipients(recipients)?
		.wrap_output(&mut encrypted)
		.expect("in memory write");
	io::copy(&mut Cursor::new(data), &mut encryptor).expect("in memory copy");
	encryptor.finish().expect("in memory flush");
	Some(SecretData {
		data: encrypted,
		encrypted: true,
	})
}

fn main() -> Result<()> {
	let opts = Opts::parse();
	// Assumed to be secure, seeded from secure OsRng+reseeded.
	let mut rng = thread_rng();

	match opts {
		Opts::Public { allow_empty } => {
			let stdin = parse_stdin()?;
			if stdin.is_none() && !allow_empty {
				bail!("empty stdin input is not allowed unless --allow-empty is set");
			}
			let stdin = stdin.unwrap_or_default();
			io::stdout().write_all(
				SecretData {
					data: stdin,
					encrypted: false,
				}
				.to_string()
				.as_bytes(),
			)?;
		}
		Opts::Private {
			allow_empty,
			recipient,
		} => {
			let stdin = parse_stdin()?;
			if stdin.is_none() && !allow_empty {
				bail!("empty stdin input is not allowed unless --allow-empty is set");
			}
			let stdin = stdin.unwrap_or_default();
			if recipient.is_empty() {
				bail!("recipient list is empty");
			}
			let out = encrypt_secret_data(
				recipient
					.into_iter()
					.map(|r| age::ssh::Recipient::from_str(&r))
					.collect::<Result<Vec<age::ssh::Recipient>, age::ssh::ParseRecipientKeyError>>()
					.map_err(|e| anyhow!("parse recipients: {e:?}"))?,
				stdin,
			)
			.expect("got recipients");
			io::stdout().write_all(out.to_string().as_bytes())?;
		}
		Opts::Generate(gen) => {
			let mut stdout_marker: bool = false;
			match gen {
				Generate::Ed25519 {
					public,
					private,
					no_embed_public,
				} => {
					let key = SigningKey::generate(&mut rng).to_keypair_bytes();

					write_output(&public, &key[32..], &mut stdout_marker).context("public")?;
					write_output(
						&private,
						&key[..{
							if no_embed_public {
								32
							} else {
								64
							}
						}],
						&mut stdout_marker,
					)
					.context("private")?;
				}
				Generate::Password {
					size,
					no_symbols,
					output,
				} => {
					ensure!(
						size >= 6,
						"misconfiguration? password is shorter than 6 chars"
					);
					let out = if no_symbols {
						Alphanumeric.sample_string(&mut rng, size)
					} else {
						// Alphabet of Alphanumberic + symbols
						const GEN_ASCII_SYMBOLS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";
						let uniform = Uniform::new(0, GEN_ASCII_SYMBOLS.len());
						(0..size)
							.map(|_| uniform.sample(&mut rng))
							.map(|i| GEN_ASCII_SYMBOLS[i] as char)
							.collect::<String>()
					};
					write_output(&output, out, &mut stdout_marker)?;
				}
			}
		}
	}
	Ok(())
}
