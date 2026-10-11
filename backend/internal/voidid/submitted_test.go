package voidid

import (
	"errors"
	"strings"
	"testing"
)

func TestValidateSubmittedRequiresNonblankID(t *testing.T) {
	for _, text := range []string{"", " ", "\t\r\n", "\u00a0\u3000"} {
		if err := ValidateSubmitted(text); !errors.Is(err, ErrInvalid) {
			t.Errorf("ValidateSubmitted(%q) = %v, want ErrInvalid", text, err)
		}
	}
}

func TestValidateSubmittedPostgresTextBoundaries(t *testing.T) {
	tests := []struct {
		name  string
		text  string
		valid bool
	}{
		{"512 ASCII bytes", strings.Repeat("a", 512), true},
		{"513 ASCII bytes", strings.Repeat("a", 513), false},
		{"512 UTF-8 bytes", strings.Repeat("界", 170) + "ab", true},
		{"513 UTF-8 bytes", strings.Repeat("界", 171), false},
		{"embedded NUL", "client\x00id", false},
		{"invalid UTF-8", "client\xffid", false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := ValidateSubmitted(tt.text)
			if tt.valid && err != nil {
				t.Fatalf("ValidateSubmitted() = %v, want nil", err)
			}
			if !tt.valid && !errors.Is(err, ErrInvalid) {
				t.Fatalf("ValidateSubmitted() = %v, want ErrInvalid", err)
			}
		})
	}
}

func TestValidateSubmittedDoesNotImposeGeneratedFormat(t *testing.T) {
	for _, text := range []string{
		"client-generated-id",
		"experiment:MixedCase_1",
		" a client ID ",
		"VOID-AAAQEAYE-AUDAOCAJ-BIFQYDIO-B4IBCEQT",
	} {
		if err := ValidateSubmitted(text); err != nil {
			t.Errorf("ValidateSubmitted(%q) = %v, want nil", text, err)
		}
	}
}
