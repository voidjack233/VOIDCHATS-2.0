package prekeys

import (
	"bytes"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"io"
	"strconv"
)

// ParseUpload rejects alternate field spellings, duplicates, nulls, unknown
// fields, noninteger numbers and noncanonical base64 before any storage changes.
// The digest covers the exact decoded upload bytes for publication retries.
func ParseUpload(payload string) (Upload, [32]byte, error) {
	var upload Upload
	var hash [32]byte
	if len(payload) == 0 || len(payload) > base64.StdEncoding.EncodedLen(MaxUploadBytes) {
		return upload, hash, ErrInvalid
	}
	raw, ok := decodeBase64(payload, 1, MaxUploadBytes)
	if !ok {
		return upload, hash, ErrInvalid
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.UseNumber()
	if err := scanValue(decoder, 0); err != nil {
		return upload, hash, ErrInvalid
	}
	if _, err := decoder.Token(); err != io.EOF {
		return upload, hash, ErrInvalid
	}
	decoder = json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&upload); err != nil || validateUpload(upload) != nil {
		return Upload{}, hash, ErrInvalid
	}
	return upload, sha256.Sum256(raw), nil
}

var uploadFields = map[string]bool{
	"v": true, "device_id": true, "registration_id": true,
	"identity_key": true, "account_signature": true,
	"signed_prekey": true, "kyber_last_resort": true,
	"one_time_prekeys": true, "one_time_kyber_prekeys": true,
	"key_id": true, "public_key": true, "signature": true,
}

func scanValue(decoder *json.Decoder, depth int) error {
	if depth > 8 {
		return ErrInvalid
	}
	token, err := decoder.Token()
	if err != nil || token == nil {
		return ErrInvalid
	}
	switch value := token.(type) {
	case json.Delim:
		switch value {
		case '{':
			seen := map[string]bool{}
			for decoder.More() {
				token, err := decoder.Token()
				name, ok := token.(string)
				if err != nil || !ok || !uploadFields[name] || seen[name] {
					return ErrInvalid
				}
				seen[name] = true
				if err := scanValue(decoder, depth+1); err != nil {
					return err
				}
			}
			end, err := decoder.Token()
			if err != nil || end != json.Delim('}') {
				return ErrInvalid
			}
		case '[':
			for decoder.More() {
				if err := scanValue(decoder, depth+1); err != nil {
					return err
				}
			}
			end, err := decoder.Token()
			if err != nil || end != json.Delim(']') {
				return ErrInvalid
			}
		default:
			return ErrInvalid
		}
	case json.Number:
		text := string(value)
		for _, c := range text {
			if c < '0' || c > '9' {
				return ErrInvalid
			}
		}
		if _, err := strconv.ParseUint(text, 10, 64); err != nil {
			return ErrInvalid
		}
	case string:
	default:
		return ErrInvalid
	}
	return nil
}

func validateUpload(upload Upload) error {
	if upload.V != 1 || upload.DeviceID != 1 || upload.RegistrationID < 1 || upload.RegistrationID > 16380 ||
		!validEC(upload.IdentityKey) || !validSignature(upload.AccountSignature) ||
		!validSigned(upload.SignedPrekey, true) || !validSigned(upload.KyberLastResort, false) ||
		upload.OneTimePrekeys == nil || upload.OneTimeKyberPrekeys == nil ||
		len(upload.OneTimePrekeys) > MaxInventoryKeys || len(upload.OneTimeKyberPrekeys) > MaxInventoryKeys {
		return ErrInvalid
	}
	ecIDs, kyberIDs := map[int]bool{}, map[int]bool{}
	for _, key := range upload.OneTimePrekeys {
		if !validID(key.KeyID) || !validEC(key.PublicKey) || ecIDs[key.KeyID] {
			return ErrInvalid
		}
		ecIDs[key.KeyID] = true
	}
	for _, key := range upload.OneTimeKyberPrekeys {
		if !validSigned(key, false) || kyberIDs[key.KeyID] || key.KeyID == upload.KyberLastResort.KeyID {
			return ErrInvalid
		}
		kyberIDs[key.KeyID] = true
	}
	return nil
}

func validID(id int) bool { return id >= 1 && id <= MaxKeyID }

func validEC(value string) bool {
	raw, ok := decodeBase64(value, 33, 33)
	return ok && raw[0] == 5
}

func validSignature(value string) bool {
	_, ok := decodeBase64(value, 64, 64)
	return ok
}

func validSigned(key SignedKey, ec bool) bool {
	if !validID(key.KeyID) || !validSignature(key.Signature) {
		return false
	}
	if ec {
		return validEC(key.PublicKey)
	}
	_, ok := decodeBase64(key.PublicKey, 1, 2048)
	return ok
}

func decodeBase64(value string, minimum, maximum int) ([]byte, bool) {
	if len(value) > base64.StdEncoding.EncodedLen(maximum) {
		return nil, false
	}
	raw, err := base64.StdEncoding.Strict().DecodeString(value)
	if err != nil || len(raw) < minimum || len(raw) > maximum || base64.StdEncoding.EncodeToString(raw) != value {
		return nil, false
	}
	return raw, true
}
