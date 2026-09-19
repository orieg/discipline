--TEST--
Comments And Strings In PHPT Test
--FILE--
<?php
// --EXPECT--
/* assert(1 == 2); */
$str = "--EXPECT--\nfoo";
echo "done";
?>
--EXPECT--
done
