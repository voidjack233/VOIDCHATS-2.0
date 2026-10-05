// Package voidid defines the stable public identifier of a VOID account.
// A VOID-ID is neither an authentication secret nor an encryption or chat
// identity key. Chat identity private keys belong only on client devices.
//
// The current text format is provisional: "VOID-" followed by four groups
// of eight uppercase RFC 4648 Base32 characters, without padding. Encoding
// decisions are kept in this package so account logic can use ID values.
package voidid

import (
	"crypto/rand"
	"encoding/base32"
	"errors"
	"fmt"
	"io"
)

const (
	entropyBytes = 20 // 160 random bits.
	textLength   = 40
)

var (
	base32Encoding = base32.StdEncoding.WithPadding(base32.NoPadding)

	// ErrInvalid reports an invalid VOID-ID value or text representation.
	ErrInvalid = errors.New("invalid VOID-ID")
)

// ID is an opaque, comparable account identifier. Its zero value is invalid;
// use New or Parse to obtain a valid value. Equality compares identity, without
// requiring callers to depend on the current text representation.
type ID struct {
	value [entropyBytes]byte
	valid bool
}

// New generates an ID with 160 bits of cryptographically secure randomness.
// It returns read errors and never falls back to insecure randomness. Modern
// Go versions may terminate the process if the operating system's secure
// random source fails; such a failure must not be recovered with a weaker source.
//
// Generation is stateless. Future account storage should enforce uniqueness
// and retry New if an extremely unlikely collision violates its constraint.
func New() (ID, error) {
	return generate(rand.Reader)
}

// generate permits testing read failures without replacing the process-wide
// secure random source. Production callers can only use New.
func generate(reader io.Reader) (ID, error) {
	var id ID
	if _, err := io.ReadFull(reader, id.value[:]); err != nil {
		return ID{}, fmt.Errorf("generate VOID-ID: %w", err)
	}
	id.valid = true
	return id, nil
}

// Parse accepts only the canonical text representation. It does not trim
// whitespace, fold case, or normalize separators.
func Parse(text string) (ID, error) {
	if len(text) != textLength || text[:5] != "VOID-" {
		return ID{}, ErrInvalid
	}
	if text[13] != '-' || text[22] != '-' || text[31] != '-' {
		return ID{}, ErrInvalid
	}

	encoded := text[5:13] + text[14:22] + text[23:31] + text[32:40]
	// encoding/base32 accepts embedded line breaks. Check the alphabet here
	// so parsing is strict and a single ID has exactly one valid spelling.
	for i := range encoded {
		c := encoded[i]
		if !(c >= 'A' && c <= 'Z' || c >= '2' && c <= '7') {
			return ID{}, ErrInvalid
		}
	}

	var id ID
	if _, err := base32Encoding.Decode(id.value[:], []byte(encoded)); err != nil {
		return ID{}, ErrInvalid
	}
	id.valid = true
	return id, nil
}

// Validate reports whether text is a canonical VOID-ID.
func Validate(text string) error {
	_, err := Parse(text)
	return err
}

// IsValid reports whether id was constructed successfully by New or Parse.
func (id ID) IsValid() bool {
	return id.valid
}

// String returns the canonical text representation, or an empty string for
// an invalid value. MarshalText returns an error for invalid values.
func (id ID) String() string {
	if !id.IsValid() {
		return ""
	}
	encoded := base32Encoding.EncodeToString(id.value[:])
	return "VOID-" + encoded[:8] + "-" + encoded[8:16] + "-" + encoded[16:24] + "-" + encoded[24:]
}

// MarshalText implements encoding.TextMarshaler and rejects invalid values.
func (id ID) MarshalText() ([]byte, error) {
	if !id.IsValid() {
		return nil, ErrInvalid
	}
	return []byte(id.String()), nil
}

// UnmarshalText implements encoding.TextUnmarshaler. Invalid text returns an
// error and leaves the receiver unchanged.
func (id *ID) UnmarshalText(text []byte) error {
	if id == nil {
		return ErrInvalid
	}
	parsed, err := Parse(string(text))
	if err != nil {
		return err
	}
	*id = parsed
	return nil
}
