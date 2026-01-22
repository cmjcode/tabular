#!/usr/bin/env python3
"""Fix backspace handling in editor.rs to skip when dialog is open"""

with open('src/editor.rs', 'r') as f:
    lines = f.readlines()

# Find and replace the three blocks
modified = []
i = 0
while i < len(lines):
    line = lines[i]
    
    # First block: "Pre-handle Delete/Backspace"
    if i < len(lines) - 1 and '// Pre-handle Delete/Backspace when a selection exists' in line:
        modified.append(line)
        i += 1
        # Next line should be the second comment
        modified.append(lines[i])  # "// This ensures expected behavior..."
        i += 1
        # Next should be "    {"
        if lines[i].strip() == '{':
            # Replace with if check
            modified.append('    // SKIP this handling if Custom View dialog is open to avoid consuming backspace events\n')
            modified.append('    if !tabular.show_add_view_dialog {\n')
            i += 1
            continue
    
    # Second block: "Special guard: Backspace on completely empty text"
    elif i < len(lines) - 1 and '// Special guard: Backspace on completely empty text' in line:
        modified.append(line)
        i += 1
        # Next should be "    {"
        if lines[i].strip() == '{':
            # Replace with if check
            modified.append('    // SKIP this handling if Custom View dialog is open\n')
            modified.append('    if !tabular.show_add_view_dialog {\n')
            i += 1
            continue
    
    # Third block: "Capture multi-cursor typing/deletion events"
    elif '// Capture multi-cursor typing/deletion events before TextEdit consumes them' in line:
        modified.append(line)
        i += 1
        # Next should be "    if tabular.multi_selection.len() > 1 {"
        if 'if tabular.multi_selection.len() > 1 {' in lines[i]:
            # Add dialog check
            modified.append('    // SKIP this when Custom View dialog is open\n')
            modified.append('    if !tabular.show_add_view_dialog && tabular.multi_selection.len() > 1 {\n')
            i += 1
            continue
    
    modified.append(line)
    i += 1

# Write back
with open('src/editor.rs', 'w') as f:
    f.writelines(modified)

print("✅ Modified editor.rs - added dialog checks to 3 backspace handlers")
