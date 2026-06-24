[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$esc = [char]27
$bel = [char]7
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)

function Write-At {
    param(
        [Parameter(Mandatory)] [int]$Row,
        [Parameter(Mandatory)] [int]$Column,
        [Parameter(Mandatory)] [string]$Text
    )

    [Console]::Out.Write("${esc}[${Row};${Column}H${Text}")
}

function Write-CellSpan {
    param(
        [Parameter(Mandatory)] [int]$Row,
        [Parameter(Mandatory)] [int]$Column,
        [Parameter(Mandatory)] [int]$Columns,
        [Parameter(Mandatory)] [int]$Rows,
        [Parameter(Mandatory)] [string]$Text,
        [switch]$UseBel
    )

    [Console]::Out.Write("${esc}[${Row};${Column}H")
    $command = "${esc}]7777;knightty;span=${Columns}x${Rows}:${Text}"
    if ($UseBel) {
        [Console]::Out.Write("${command}${bel}")
    }
    else {
        [Console]::Out.Write("${command}${esc}\")
    }
}

[Console]::Out.Write("${esc}[2J${esc}[H")
Write-At -Row 1 -Column 1 -Text "Knightty Cell Span smoke test (minimum grid: 40x18)"

Write-At -Row 3 -Column 1 -Text "ASCII 4x2"
Write-CellSpan -Row 3 -Column 18 -Columns 4 -Rows 2 -Text "A"

Write-At -Row 6 -Column 1 -Text "CJK 5x3"
Write-CellSpan -Row 6 -Column 18 -Columns 5 -Rows 3 -Text "界" -UseBel

Write-At -Row 10 -Column 1 -Text "Combining 6x2"
Write-CellSpan -Row 10 -Column 18 -Columns 6 -Rows 2 -Text "e$([char]0x0301)"

Write-At -Row 13 -Column 1 -Text "ZWJ emoji 6x3"
Write-CellSpan -Row 13 -Column 18 -Columns 6 -Rows 3 -Text "👨‍💻"

Write-At -Row 17 -Column 1 -Text "Wide rectangle 10x1"
Write-CellSpan -Row 17 -Column 22 -Columns 10 -Rows 1 -Text "W"

[Console]::Out.Write("${esc}[18;1H")
