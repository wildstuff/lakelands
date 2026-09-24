# Test certificates

For `tests/tls.rs` only; nothing here protects anything.

- `ca.pem`: a test certificate authority.
- `server.pem`, `server.key`: a server certificate for `localhost`,
  signed by `ca.pem`, and its key.
- `other-ca.pem`: an unrelated authority, which does not vouch for
  `server.pem`.

All are P-256 and valid until 2126. The authorities' keys were not
kept. To make them again:

```sh
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout ca.key -out ca.pem -days 36500 -subj "/CN=lakelands test CA" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign"
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout server.key -out server.csr -subj "/CN=localhost"
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -days 36500 \
  -extfile <(printf "subjectAltName=DNS:localhost\nextendedKeyUsage=serverAuth\n") \
  -out server.pem
```
