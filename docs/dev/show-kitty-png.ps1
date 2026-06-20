[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string]$Path,

    [ValidateRange(1, 65535)]
    [int]$Columns = 40,

    [ValidateRange(0, 65535)]
    [int]$Rows = 0,

    [uint32]$ImageId = 42
)

$ErrorActionPreference = "Stop"

if ($ImageId -eq 0) {
    throw "ImageId must be greater than zero."
}

if ($Path) {
    $resolvedPath = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).ProviderPath
    $bytes = [IO.File]::ReadAllBytes($resolvedPath)
    $signature = [byte[]](137, 80, 78, 71, 13, 10, 26, 10)
    if ($bytes.Length -lt $signature.Length) {
        throw "The selected file is not a PNG image."
    }
    for ($index = 0; $index -lt $signature.Length; $index++) {
        if ($bytes[$index] -ne $signature[$index]) {
            throw "The selected file is not a PNG image."
        }
    }
    $png = [Convert]::ToBase64String($bytes)
}
else {
    # Opaque 1x1 red PNG used when no path is supplied.
    $png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR42mP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC"
}

$esc = [char]27
$chunkSize = 4096
$first = $true

for ($offset = 0; $offset -lt $png.Length; $offset += $chunkSize) {
    $length = [Math]::Min($chunkSize, $png.Length - $offset)
    $chunk = $png.Substring($offset, $length)
    if ($first) {
        $control = "a=T,f=100,t=d,i=${ImageId},p=1,c=${Columns}"
        if ($Rows -gt 0) {
            $control += ",r=${Rows}"
        }
        $control += ",C=0,q=2,m=1"
        $first = $false
    }
    else {
        $control = "m=1"
    }
    [Console]::Out.Write("${esc}_G${control};${chunk}${esc}\")
}

# An empty final chunk makes even small PNGs exercise the multipart path.
[Console]::Out.Write("${esc}_Gm=0;${esc}\")
[Console]::Out.WriteLine("Kitty multipart PNG display complete.")
