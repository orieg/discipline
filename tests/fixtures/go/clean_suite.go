package main

import (
	"testing"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestAddition(t *testing.T) {
	sum := 20 + 22
	if sum != 42 {
		t.Fatalf("expected 42, got %d", sum)
	}
}

func TestWithTestify(t *testing.T) {
	expected := 100
	actual := 50 * 2
	assert.Equal(t, expected, actual)
	require.NotNil(t, t)
}

func TestSubtests(t *testing.T) {
	t.Run("sub1", func(t *testing.T) {
		val := "hello"
		assert.Equal(t, "hello", val)
	})
}
