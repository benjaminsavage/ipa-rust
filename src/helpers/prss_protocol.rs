use crate::helpers::messaging::{Gateway, Message};
use crate::helpers::{Direction, Error, MESSAGE_PAYLOAD_SIZE_BYTES};
use crate::protocol::{prss, RecordId, Step, Substep};
use rand_core::{CryptoRng, RngCore};
use std::io::ErrorKind;
use std::iter::zip;
use tinyvec::ArrayVec;
use x25519_dalek::PublicKey;

struct PrssExchangeStep;

impl AsRef<str> for PrssExchangeStep {
    fn as_ref(&self) -> &str {
        "prss_exchange"
    }
}

impl Substep for PrssExchangeStep {}

/// establish the prss endpoint by exchanging public keys with the other helpers
/// # Errors
/// if communication with other helpers fails
pub async fn negotiate<R: RngCore + CryptoRng>(
    gateway: &Gateway,
    step: &Step,
    rng: &mut R,
) -> Result<prss::Endpoint, Error> {
    // setup protocol to exchange prss public keys
    let step = step.narrow(&PrssExchangeStep);
    let channel = gateway.mesh(&step);

    let left_peer = gateway.role().peer(Direction::Left);
    let right_peer = gateway.role().peer(Direction::Right);

    // setup local prss endpoint
    let ep_setup = prss::Endpoint::prepare(rng);
    let (send_left_pk, send_right_pk) = ep_setup.public_keys();
    let send_left_pk_chunks = PublicKeyChunk::chunks(send_left_pk);
    let send_right_pk_chunks = PublicKeyChunk::chunks(send_right_pk);

    // exchange public keys
    // TODO: since we have a limitation that max message size is 8 bytes, we must send 4
    //       messages to completely send the public key. If that max message size is removed, we
    //       can eliminate the chunking
    let mut recv_left_pk_builder = PublicKeyBytesBuilder::empty();
    let mut recv_right_pk_builder = PublicKeyBytesBuilder::empty();

    for (i, (send_left_chunk, send_right_chunk)) in
        zip(send_left_pk_chunks, send_right_pk_chunks).enumerate()
    {
        let record_id = RecordId::from(i);
        let send_to_left = channel.send(left_peer, record_id, send_left_chunk);
        let send_to_right = channel.send(right_peer, record_id, send_right_chunk);
        let recv_from_left = channel.receive::<PublicKeyChunk>(left_peer, record_id);
        let recv_from_right = channel.receive::<PublicKeyChunk>(right_peer, record_id);
        let (_, _, recv_left_key_chunk, recv_right_key_chunk) =
            tokio::try_join!(send_to_left, send_to_right, recv_from_left, recv_from_right)?;
        recv_left_pk_builder.append_chunk(recv_left_key_chunk);
        recv_right_pk_builder.append_chunk(recv_right_key_chunk);
    }

    let recv_left_pk = recv_left_pk_builder
        .build()
        .map_err(|err| Error::serialization_error(err.record_id(), &step, err))?;
    let recv_right_pk = recv_right_pk_builder
        .build()
        .map_err(|err| Error::serialization_error(err.record_id(), &step, err))?;

    Ok(ep_setup.setup(&recv_left_pk, &recv_right_pk))
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
#[error("missing {} chunks when trying to build public key", PublicKeyBytesBuilder::FULL_COUNT - incomplete_count)]
pub struct IncompletePublicKey {
    incomplete_count: u8,
}

impl IncompletePublicKey {
    #[must_use]
    pub fn record_id(&self) -> RecordId {
        RecordId::from(u32::from(self.incomplete_count))
    }
}

#[derive(Debug, Default, PartialEq, Eq, Copy, Clone)]
pub struct PublicKeyChunk([u8; 8]);

impl PublicKeyChunk {
    pub fn chunks(pk: PublicKey) -> [PublicKeyChunk; 4] {
        let pk_bytes = pk.to_bytes();

        // These assumptions are necessary for ser/de to work
        assert_eq!(MESSAGE_PAYLOAD_SIZE_BYTES, 8);
        assert_eq!(pk_bytes.len(), 32);

        pk_bytes
            .chunks(MESSAGE_PAYLOAD_SIZE_BYTES)
            .map(|chunk| {
                let mut chunk_bytes = [0u8; MESSAGE_PAYLOAD_SIZE_BYTES];
                chunk_bytes.copy_from_slice(chunk);
                PublicKeyChunk(chunk_bytes)
            })
            .collect::<ArrayVec<[PublicKeyChunk; 4]>>()
            .into_inner()
    }

    pub fn into_inner(self) -> [u8; MESSAGE_PAYLOAD_SIZE_BYTES] {
        self.0
    }
}

impl Message for PublicKeyChunk {
    #[allow(clippy::cast_possible_truncation)]
    const SIZE_IN_BYTES: u32 = MESSAGE_PAYLOAD_SIZE_BYTES as u32;

    fn deserialize(buf: &mut [u8]) -> std::io::Result<Self> {
        if Self::SIZE_IN_BYTES as usize <= buf.len() {
            let mut chunk = [0; Self::SIZE_IN_BYTES as usize];
            chunk.copy_from_slice(&buf[..Self::SIZE_IN_BYTES as usize]);
            Ok(PublicKeyChunk(chunk))
        } else {
            Err(std::io::Error::new(
                ErrorKind::UnexpectedEof,
                format!(
                    "expected buffer of size {}, but it was of size {}",
                    Self::SIZE_IN_BYTES,
                    buf.len()
                ),
            ))
        }
    }

    fn serialize(self, buf: &mut [u8]) -> std::io::Result<()> {
        if buf.len() >= self.0.len() {
            buf[..Self::SIZE_IN_BYTES as usize].copy_from_slice(&self.0);
            Ok(())
        } else {
            Err(std::io::Error::new(
                ErrorKind::WriteZero,
                format!(
                    "expected buffer to be at least {} bytes, but was only {} bytes",
                    Self::SIZE_IN_BYTES,
                    buf.len()
                ),
            ))
        }
    }
}

#[derive(Debug, Default)]
pub struct PublicKeyBytesBuilder {
    bytes: ArrayVec<[u8; 32]>,
    count: u8,
}

impl PublicKeyBytesBuilder {
    const FULL_COUNT: u8 = 4;

    pub fn empty() -> Self {
        PublicKeyBytesBuilder {
            bytes: ArrayVec::new(),
            count: 0,
        }
    }
    pub fn append_chunk(&mut self, chunk: PublicKeyChunk) {
        self.bytes.extend_from_slice(&chunk.into_inner());
        self.count += 1;
    }
    pub fn build(self) -> Result<PublicKey, IncompletePublicKey> {
        if self.count == PublicKeyBytesBuilder::FULL_COUNT {
            Ok(self.bytes.into_inner().into())
        } else {
            Err(IncompletePublicKey {
                incomplete_count: self.count,
            })
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use rand::thread_rng;
    use x25519_dalek::{EphemeralSecret, PublicKey};

    #[test]
    fn chunk_ser_de() {
        let chunk_bytes = [1, 2, 3, 4, 5, 6, 7, 8];
        let chunk = PublicKeyChunk(chunk_bytes);

        let mut serialized = [0u8; 8];
        chunk.serialize(&mut serialized).unwrap();
        assert_eq!(chunk_bytes, serialized);

        let deserialized = PublicKeyChunk::deserialize(&mut serialized).unwrap();
        assert_eq!(chunk, deserialized);
    }

    #[test]
    fn incomplete_pk() {
        let secret = EphemeralSecret::new(thread_rng());
        let pk = PublicKey::from(&secret);

        let chunks = PublicKeyChunk::chunks(pk);

        // check incomplete keys fail
        for i in 0..chunks.len() {
            let mut builder = PublicKeyBytesBuilder::empty();
            for chunk in chunks.iter().take(i) {
                builder.append_chunk(*chunk);
            }
            let built = builder.build();
            let expected_err = Err(IncompletePublicKey {
                incomplete_count: u8::try_from(i).unwrap(),
            });
            assert_eq!(built, expected_err);
        }

        // check complete key succeeds
        let mut builder = PublicKeyBytesBuilder::empty();
        for chunk in chunks {
            builder.append_chunk(chunk);
        }
        assert_eq!(builder.build(), Ok(pk));
    }
}
