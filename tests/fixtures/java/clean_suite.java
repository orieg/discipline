package com.example.discipline;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.ValueSource;
import static org.junit.jupiter.api.Assertions.*;

public class CleanSuiteTest {

    @Test
    public void testCalculation() {
        int result = 40 + 2;
        assertEquals(42, result);
        assertTrue(result > 0);
    }

    @ParameterizedTest
    @ValueSource(ints = {1, 2, 3})
    public void testParameterized(int val) {
        assertNotNull(val);
        assertEquals(val, val * 1);
    }
}
