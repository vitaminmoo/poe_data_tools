$OUT_FOLDER = "E:\programming\data\poe\file_dumps"

# Extract the dat64 files from the cdn
#& cargo run --release --bin poe_files -- extract "$OUT_FOLDER" "*.datc64"

# Extract the dds files from the steam folder
#& cargo run --release --bin poe_files -- --steam extract "$OUT_FOLDER" "*.dds"


# Use imageMagick to turn all the dds files into png
$PNG_FOLDER = "E:\programming\data\poe\dds_pngs"
Get-ChildItem -Path $OUT_FOLDER -Recurse -Filter "*.dds" | ForEach-Object {
    # Get the full path of the .dds file
    $ddsPath = $_.FullName

    # Construct the corresponding .png path
    $relativePath = $ddsPath.Substring($OUT_FOLDER.Length + 1)  # Remove $OUT_FOLDER prefix
    $pngPath = Join-Path $PNG_FOLDER ([System.IO.Path]::ChangeExtension($relativePath, ".png"))

    # Ensure the output directory exists
    $pngDirectory = Split-Path $pngPath
    if (!(Test-Path -Path $pngDirectory)) {
        New-Item -ItemType Directory -Path $pngDirectory | Out-Null
    }

    # Print the conversion progress
    Write-Output "Converting: $ddsPath -> $pngPath"

    # Convert .dds to .png using ImageMagick
    magick $ddsPath $pngPath
}
