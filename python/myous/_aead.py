"""The two AEADs and HKDF-SHA256 the protocol uses, from `cryptography`
where it is installed and from pycryptodome where it isn't: on Intel Macs,
where cryptography has no wheels (RELEASING.md, "The Intel Mac path").
cryptography's OpenSSL uses the CPU's AES and carry-less multiply
instructions on ARM as on x86; pycryptodome's are constant-time on x86
(AES-NI, CLMUL) but table-based in its ARM wheels, so it is only the
fallback. Both give the same bytes (tests/aead_test.py).

A sealed message is the ciphertext with its 16-byte tag appended. Sizes
are the callers' to check: these take what the library takes. A wrong key,
nonce or tag raises an exception, whose type depends on the backend."""
try:
    from cryptography.hazmat.primitives import hashes
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM, ChaCha20Poly1305
    from cryptography.hazmat.primitives.kdf.hkdf import HKDF
except ImportError:
    from Crypto.Cipher import AES, ChaCha20_Poly1305
    from Crypto.Hash import SHA256
    from Crypto.Protocol.KDF import HKDF as _HKDF

    BACKEND = "pycryptodome"
    _TAG = 16

    def aes_gcm_seal(key: bytes, nonce: bytes, plaintext: bytes) -> bytes:
        body, tag = AES.new(key, AES.MODE_GCM, nonce=nonce, mac_len=_TAG).encrypt_and_digest(plaintext)
        return body + tag

    def aes_gcm_open(key: bytes, nonce: bytes, sealed: bytes) -> bytes:
        cipher = AES.new(key, AES.MODE_GCM, nonce=nonce, mac_len=_TAG)
        return cipher.decrypt_and_verify(sealed[:-_TAG], sealed[-_TAG:])

    def _chacha(key: bytes, nonce: bytes, aad: bytes):
        cipher = ChaCha20_Poly1305.new(key=key, nonce=nonce)
        cipher.update(aad)
        return cipher

    def chacha20_poly1305_seal(key: bytes, nonce: bytes, aad: bytes, plaintext: bytes) -> bytes:
        body, tag = _chacha(key, nonce, aad).encrypt_and_digest(plaintext)
        return body + tag

    def chacha20_poly1305_open(key: bytes, nonce: bytes, aad: bytes, sealed: bytes) -> bytes:
        return _chacha(key, nonce, aad).decrypt_and_verify(sealed[:-_TAG], sealed[-_TAG:])

    def _hkdf(key: bytes, length: int, info: bytes) -> bytes:
        return _HKDF(key, length, bytes(SHA256.digest_size), SHA256, context=info)
else:
    BACKEND = "cryptography"

    def aes_gcm_seal(key: bytes, nonce: bytes, plaintext: bytes) -> bytes:
        return AESGCM(key).encrypt(nonce, plaintext, None)

    def aes_gcm_open(key: bytes, nonce: bytes, sealed: bytes) -> bytes:
        return AESGCM(key).decrypt(nonce, sealed, None)

    def chacha20_poly1305_seal(key: bytes, nonce: bytes, aad: bytes, plaintext: bytes) -> bytes:
        return ChaCha20Poly1305(key).encrypt(nonce, plaintext, aad)

    def chacha20_poly1305_open(key: bytes, nonce: bytes, aad: bytes, sealed: bytes) -> bytes:
        return ChaCha20Poly1305(key).decrypt(nonce, sealed, aad)

    def _hkdf(key: bytes, length: int, info: bytes) -> bytes:
        return HKDF(algorithm=hashes.SHA256(), length=length, salt=None, info=info).derive(key)


def hkdf_sha256(key: bytes, length: int, info: bytes) -> bytes:
    """No salt, which RFC 5869 defines as HashLen zero bytes. The key must
    be bytes in both backends: pycryptodome's HMAC would take None as an
    empty message, where cryptography raises."""
    if not isinstance(key, (bytes, bytearray)):
        raise TypeError(f"hkdf_sha256 takes the key as bytes, not {type(key).__name__}")
    return _hkdf(key, length, info)
