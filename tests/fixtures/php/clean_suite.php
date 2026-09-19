<?php

use PHPUnit\Framework\TestCase;

class CleanSuiteTest extends TestCase
{
    public function testAddition()
    {
        $sum = 20 + 22;
        $this->assertEquals(42, $sum);
        $this->assertTrue($sum > 0);
    }

    public function testStringOperations()
    {
        $text = "hello" . " world";
        $this->assertSame("hello world", $text);
    }
}
