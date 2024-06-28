use std::{
	env,
	fs::{File, OpenOptions},
	io::{copy, Read, Write},
	str::FromStr,
};

use age::{
	ssh::{ParseRecipientKeyError, Recipient as SshRecipient},
	Encryptor, Recipient,
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use clap::{Parser, ValueEnum};
use fleet_shared::SecretData;
use rand::{
	distributions::{Alphanumeric, DistString, Distribution, Uniform},
	thread_rng,
};

fn write_output_file(out: &str) -> Result<File> {
	let file = OpenOptions::new()
		.create_new(true)
		.write(true)
		.open(out)
		.with_context(|| format!("failed to open output {out:?}"))?;
	Ok(file)
}
fn write_public(out: &str, mut input: impl Read, encoding: OutputEncoding) -> Result<()> {
	let mut output = write_output_file(out)?;

	let mut data = Vec::new();
	copy(&mut input, &mut wrap_encoder(&mut data, encoding))?;

	output.write_all(
		SecretData {
			data,
			encrypted: false,
		}
		.to_string()
		.as_bytes(),
	)?;
	Ok(())
}
fn write_private(
	identities: &Identities,
	out: &str,
	mut input: impl Read,
	encoding: OutputEncoding,
) -> Result<()> {
	let mut output = write_output_file(out)?;
	let encryptor = make_encryptor(identities)?;

	let mut data = Vec::new();
	{
		let mut encrypted_writer = encryptor.wrap_output(&mut data)?;
		copy(
			&mut input,
			&mut wrap_encoder(&mut encrypted_writer, encoding),
		)?;
		encrypted_writer.finish()?;
	};

	output.write_all(
		SecretData {
			data,
			encrypted: true,
		}
		.to_string()
		.as_bytes(),
	)?;
	Ok(())
}

type Identities = Vec<SshRecipient>;
fn load_identities() -> Result<Identities> {
	let list = env::var("GENERATOR_HELPER_IDENTITIES");
	let list = match list {
		Ok(v) => v,
		Err(env::VarError::NotPresent) => {
			bail!("gh is only intended to be used from secret generator scripts, but if you really want to use it somewhere else - set GENERATOR_HELPER_IDENTITIES to list of newline-delimited ssh identities");
		}
		Err(e) => bail!("somehow, identities list is not utf-8: {e}"),
	};
	let list = list.trim();
	ensure!(!list.is_empty(), "no identities passed, can't encrypt data");
	list.lines()
		.map(age::ssh::Recipient::from_str)
		.collect::<Result<Identities, ParseRecipientKeyError>>()
		.map_err(|e| anyhow!("parse recipients: {e:?}"))
}
fn make_encryptor(r: &Identities) -> Result<Encryptor> {
	Ok(Encryptor::with_recipients(
		r.iter()
			.map(|v| {
				let coerced: Box<dyn Recipient + Send> = Box::new(v.clone());
				coerced
			})
			.collect(),
	)
	.expect("list is not empty"))
}
fn wrap_encoder<'t>(w: impl Write + 't, encoding: OutputEncoding) -> impl Write + 't {
	fn coerce<'t>(w: impl Write + 't) -> Box<dyn Write + 't> {
		Box::new(w)
	}
	match encoding {
		OutputEncoding::Raw => coerce(w),
		OutputEncoding::Base64 => {
			use base64::engine::general_purpose::STANDARD;
			let writer = base64::write::EncoderWriter::new(w, &STANDARD);
			coerce(writer)
		}
	}
}

#[derive(Clone, Copy, ValueEnum, Default)]
enum OutputEncoding {
	/// Do not encode data, store as is.
	#[default]
	Raw,
	/// Encode as base64 (with padding).
	Base64,
}

#[derive(Parser)]
enum Generate {
	/// Generate public, private keys without wrapping, in standard ed25519 schema
	/// (64 bytes private (due to merge with private), 32 bytes public)
	Ed25519 {
		#[arg(long, short = 'p')]
		public: String,
		#[arg(long, short = 's')]
		private: String,
		/// Private key should be just the private key (32 bytes), not standard private+public.
		#[arg(long)]
		no_embed_public: bool,
		#[arg(long, short = 'e', value_enum, default_value_t)]
		encoding: OutputEncoding,
	},
	/// Generate public, private keys without wrapping, in standard x25519 schema
	/// (32 bytes private, 32 bytes public)
	X25519 {
		#[arg(long, short = 'p')]
		public: String,
		#[arg(long, short = 's')]
		private: String,
		#[arg(long, short = 'e', value_enum, default_value_t)]
		encoding: OutputEncoding,
	},
	Password {
		#[arg(long, short = 'o')]
		output: String,
		#[arg(long)]
		size: usize,
		#[arg(long, short = 'n')]
		no_symbols: bool,
		#[arg(long, short = 'e', value_enum, default_value_t)]
		encoding: OutputEncoding,
	},
}

#[derive(Parser)]
enum Opts {
	/// Encode public part from stdin.
	Public {
		#[arg(long, short = 'o')]
		output: String,
		#[arg(long, short = 'e', value_enum, default_value_t)]
		encoding: OutputEncoding,
	},
	/// Encrypt private part from stdin.
	Private {
		#[arg(long, short = 'o')]
		output: String,
		#[arg(long, short = 'e', value_enum, default_value_t)]
		encoding: OutputEncoding,
	},
	/// Generate keys in well-known schemas.
	///
	/// Note that this command is only intended to be used in fleet secret generator,
	/// otherwise you should ensure noone is able to read generated files, they don't have any mode set by default.
	#[command(subcommand)]
	Generate(Generate),
}

fn main() -> Result<()> {
	let opts = Opts::parse();
	// Assumed to be secure, seeded from secure OsRng+reseeded.
	let mut rng = thread_rng();

	match opts {
		Opts::Public { output, encoding } => {
			write_public(&output, std::io::stdin(), encoding)?;
		}
		Opts::Private { output, encoding } => {
			let recipients = load_identities()?;
			write_private(&recipients, &output, std::io::stdin(), encoding)?;
		}
		Opts::Generate(gen) => {
			match gen {
				Generate::Ed25519 {
					public,
					private,
					no_embed_public,
					encoding,
				} => {
					let recipients = load_identities()?;
					let key = ed25519_dalek::SigningKey::generate(&mut rng).to_keypair_bytes();
					write_public(&public, &key[32..], encoding)?;
					write_private(
						&recipients,
						&private,
						&key[..{
							if no_embed_public {
								32
							} else {
								64
							}
						}],
						encoding,
					)?;
				}
				Generate::X25519 {
					public,
					private,
					encoding,
				} => {
					let recipients = load_identities()?;
					let key = x25519_dalek::StaticSecret::random_from_rng(rng);
					let public_key: x25519_dalek::PublicKey = (&key).into();
					write_public(&public, public_key.as_bytes().as_slice(), encoding)?;
					write_private(&recipients, &private, key.as_bytes().as_slice(), encoding)?;
				}
				Generate::Password {
					size,
					no_symbols,
					output,
					encoding,
				} => {
					ensure!(
						size >= 6,
						"misconfiguration? password is shorter than 6 chars"
					);
					let recipients = load_identities()?;
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
					write_private(&recipients, &output, out.as_bytes(), encoding)?;
				}
			}
		}
	}
	Ok(())
}
