package voidid

import (
	"errors"
	"testing"
)

func TestValidateSubmittedRequiresNonblankID(t *testing.T) {
	for _, text := range []string{"", " ", "\t\r\n", "\u00a0\u3000"} {
		if err := ValidateSubmitted(text); !errors.Is(err, ErrInvalid) {
			t.Errorf("ValidateSubmitted(%q) = %v, want ErrInvalid", text, err)
		}
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
