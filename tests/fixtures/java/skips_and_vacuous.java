package com.example.discipline;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Disabled;
import static org.junit.jupiter.api.Assertions.*;

public class SkipsAndVacuousTest {

    @Disabled("Flaky test disabled pending investigation")
    @Test
    public void testDisabled() {
        assertEquals(1, 1);
    }

    @Test
    public void testVacuousTautology() {
        assertTrue(true);
    }

    @Test
    public void testEmpty() {
        // empty test with no assertions
    }
}
