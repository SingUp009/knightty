param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("ConsoleWrite", "ConsoleOutWrite", "StandardOutput")]
    [string]$Method
)

$ErrorActionPreference = "Stop"
$esc = [char]27
$png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
$sequence = "${esc}_Ga=T,f=100,t=d,i=4242,p=7,q=0,m=0,c=1,r=1,C=1;${png}${esc}\"
$bytes = [System.Text.Encoding]::ASCII.GetBytes($sequence)

[Console]::Write("BEGIN:{0}`n", $Method)
switch ($Method) {
    "ConsoleWrite" {
        [Console]::Write($sequence)
    }
    "ConsoleOutWrite" {
        [Console]::Out.Write($sequence)
    }
    "StandardOutput" {
        $stdout = [Console]::OpenStandardOutput()
        $stdout.Write($bytes, 0, $bytes.Length)
        $stdout.Flush()
    }
}
[Console]::Write("`nEND:{0}`n", $Method)
