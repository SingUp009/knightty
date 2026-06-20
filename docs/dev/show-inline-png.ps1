$ErrorActionPreference = "Stop"

$png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
$esc = [char]27
$sequence = "${esc}]1337;File=name=c21va2UucG5n;inline=1;width=8:${png}${esc}\"

[Console]::Out.Write($sequence)
[Console]::Out.WriteLine("Knightty inline PNG smoke test complete.")
