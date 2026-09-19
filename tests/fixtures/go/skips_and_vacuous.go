package main

import (
	"testing"
	"github.com/stretchr/testify/assert"
)

func TestSkipped(t *testing.T) {
	t.Skip("skipping in CI")
	assert.Equal(t, 1, 1)
}

func TestTautology(t *testing.T) {
	assert.True(t, true)
}

func TestEmpty(t *testing.T) {
	// Empty ghost test
}
