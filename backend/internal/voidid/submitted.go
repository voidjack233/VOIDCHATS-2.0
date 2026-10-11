package voidid

import (
	"strings"
	"unicode/utf8"
)

// ValidateSubmitted checks a client-supplied account identifier for this
// experimental milestone. Values must be nonblank valid UTF-8, contain no NUL,
// and fit within 512 bytes for PostgreSQL text and index compatibility. No
// generated-ID format is imposed; preserve the exact submitted text.
// Future account identifier rules belong here, separate from account logic.
func ValidateSubmitted(text string) error {
	if strings.TrimSpace(text) == "" || !utf8.ValidString(text) || len(text) > 512 || strings.ContainsRune(text, '\x00') {
		return ErrInvalid
	}
	return nil
}
