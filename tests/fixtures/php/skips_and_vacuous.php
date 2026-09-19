<?php

use PHPUnit\Framework\TestCase;

class SkipsAndVacuousTest extends TestCase
{
    public function testSkippedMethod()
    {
        $this->markTestSkipped("Temporarily skipped");
        $this->assertEquals(1, 1);
    }

    public function testTautology()
    {
        $this->assertTrue(true);
    }

    public function testEmptyMethod()
    {
        // Empty method with zero assertions
    }
}
