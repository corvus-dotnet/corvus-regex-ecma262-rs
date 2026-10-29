<#
.SYNOPSIS
    Writes docs/api-c.md, the API reference of the C library the documentation site publishes, from
    corvus-regex-ecma262-capi/include/corvus_regex_ecma262.h.

.DESCRIPTION
    The header's conventions (its opening comment), then its sections in order, each with its introduction and each
    declaration in it with the comment written above it: a constant (with the constants after it that have no comment
    of their own), a type or a function.

.PARAMETER Check
    Compares what would be written with docs/api-c.md and writes nothing. The script fails when they differ.
#>
param([switch] $Check)

$ErrorActionPreference = 'Stop'
$root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$headerPath = Join-Path $root 'corvus-regex-ecma262-capi' 'include' 'corvus_regex_ecma262.h'
$out = Join-Path $root 'docs' 'api-c.md'
$lines = [System.IO.File]::ReadAllText($headerPath) -replace "`r", '' -split "`n"

# The text of a comment's lines, without /*, the leading * of each line and */.
function Get-CommentLines([string[]]$comment) {
    return @($comment | ForEach-Object { $_ -replace '^\s*/\*\s?', '' -replace '\s*\*/\s*$', '' -replace '^\s*\* ?', '' -replace '^\s*\*$', '' })
}

# A comment as Markdown: its prose escaped, so that $` and $<name> are not read as code or HTML.
function Convert-Comment([string[]]$text) {
    return (($text | ForEach-Object { $_ -replace '([\\`*<\[\]])', '\$1' }) -join "`n").Trim()
}

# The opening comment: the conventions, its bullets after "The conventions" up to the licence.
$opening = @()
for ($i = 0; $lines[$i] -notmatch '\*/'; $i++) { $opening += $lines[$i] }
$opening = @(Get-CommentLines $opening)
$from = [Array]::FindIndex($opening, [Predicate[string]] { param($l) $l -match '^The conventions' }) + 1
$to = [Array]::FindIndex($opening, [Predicate[string]] { param($l) $l -match '^Licensed' })
$conventions = Convert-Comment ($opening[$from..($to - 1)])

$sections = [System.Collections.Generic.List[object]]::new()
$section = [pscustomobject]@{ Title = 'Constants and types'; Intro = ''; Entries = [System.Collections.Generic.List[object]]::new() }
$sections.Add($section)
$comment = $null
$start = [Array]::FindIndex($lines, [Predicate[string]] { param($l) $l -match '^extern "C" \{' }) + 2
for ($i = $start; $i -lt $lines.Count; $i++) {
    $line = $lines[$i]
    if ($line -match '^#ifdef __cplusplus') { break }
    if ($line.Trim() -eq '') { $comment = $null; continue }
    if ($line -match '^/\* -{10,}') {
        # A section: its title, then its introduction.
        $body = @()
        for ($i++; $lines[$i] -notmatch '\*/'; $i++) { $body += $lines[$i] }
        $body += $lines[$i]
        $body = @(Get-CommentLines $body)
        $section = [pscustomobject]@{ Title = $body[0].Trim(); Intro = (Convert-Comment ($body | Select-Object -Skip 1)); Entries = [System.Collections.Generic.List[object]]::new() }
        $sections.Add($section)
        $comment = $null
        continue
    }
    if ($line -match '^\s*/\*') {
        $block = @($line)
        while ($lines[$i] -notmatch '\*/') { $i++; $block += $lines[$i] }
        $comment = @(Get-CommentLines $block)
        continue
    }
    if ($line -match '^#define (\w+)') {
        $name = $Matches[1]
        $last = if ($section.Entries.Count) { $section.Entries[$section.Entries.Count - 1] } else { $null }
        if (-not $comment -and $last -and $last.Kind -eq 'define' -and $last.Open) {
            $last.Names += $name
            $last.Code += "`n$line"
        } else {
            $section.Entries.Add([pscustomobject]@{ Kind = 'define'; Names = @($name); Code = $line; Doc = $comment; Open = $true })
        }
        $comment = $null
        continue
    }
    # A type or a function: its lines up to the semicolon that ends it (a struct's closing line for a struct).
    $decl = @($line)
    if ($line -match '^typedef struct \w+ \{') {
        while ($lines[$i] -notmatch '^\} \w+;') { $i++; $decl += $lines[$i] }
    } else {
        while (-not $lines[$i].TrimEnd().EndsWith(';')) { $i++; $decl += $lines[$i] }
    }
    $code = $decl -join "`n"
    $name = if ($code -match '(?s)^typedef .*?(\w+);\s*$') { $Matches[1] } elseif ($code -match '(\w+)\(') { $Matches[1] } else { throw "No name in: $code" }
    foreach ($e in $section.Entries) { $e.Open = $false }
    $section.Entries.Add([pscustomobject]@{ Kind = 'decl'; Names = @($name); Code = $code; Doc = $comment; Open = $false })
    $comment = $null
}

$sb = [System.Text.StringBuilder]::new()
function Add([string]$line = '') { [void]$sb.Append($line).Append("`n") }
Add '# API reference'
Add
Add 'The C library: the header `corvus_regex_ecma262.h`, generated from its declarations and their comments by'
Add '`tools/docs/gen-api-c.ps1`. [Getting started](getting-started-c.md) shows it at work.'
Add
Add '## Conventions'
Add
Add $conventions
foreach ($s in $sections) {
    if ($s.Entries.Count -eq 0) { continue }
    Add
    Add "## $($s.Title)"
    if ($s.Intro) { Add; Add $s.Intro }
    foreach ($e in $s.Entries) {
        Add
        Add "### $($e.Names -join ', ')"
        Add
        Add '```c'
        Add $e.Code
        Add '```'
        if ($e.Doc) { Add; Add (Convert-Comment $e.Doc) }
    }
}
$text = $sb.ToString().TrimEnd("`n") + "`n"

if ($Check) {
    $current = if (Test-Path $out) { [System.IO.File]::ReadAllText($out) } else { '' }
    if ($current -ne $text) { throw 'docs/api-c.md is not what the header makes: run tools/docs/gen-api-c.ps1.' }
    Write-Host 'docs/api-c.md is current.'
    return
}
[System.IO.File]::WriteAllText($out, $text, [System.Text.UTF8Encoding]::new($false))
Write-Host 'Wrote docs/api-c.md.'
