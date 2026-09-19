--TEST--
Implicit Expectf Pattern Match PHPT Test
--FILE--
<?php
printf("Error at memory address %p in %s\n", 0x12345, __FILE__);
?>
--EXPECTF--
Error at memory address %s in %s
