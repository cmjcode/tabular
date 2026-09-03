#!/usr/bin/env python3
"""
Tabular Xcode Project & Schemes Generator
Generates a complete, standard Tabular.xcodeproj for iOS (iPadOS) and macOS App Store Connect publishing.
"""

import os
import re
import sys

def get_version(client_dir):
    cargo_toml = os.path.join(client_dir, "Cargo.toml")
    if os.path.exists(cargo_toml):
        with open(cargo_toml, "r", encoding="utf-8") as f:
            for line in f:
                m = re.match(r'^version\s*=\s*"([^"]+)"', line.strip())
                if m:
                    return m.group(1)
    return "0.14.3"

def main():
    client_dir = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    xcode_dir = os.path.join(client_dir, "Tabular.xcodeproj")
    schemes_dir = os.path.join(xcode_dir, "xcshareddata", "xcschemes")
    os.makedirs(schemes_dir, exist_ok=True)

    version = get_version(client_dir)

    # UUIDs
    # Project & Roots
    proj_uuid = "100000000000000000000001"
    main_group_uuid = "100000000000000000000002"
    products_group_uuid = "100000000000000000000003"
    apple_group_uuid = "100000000000000000000004"
    ios_group_uuid = "100000000000000000000005"
    macos_group_uuid = "100000000000000000000006"
    scripts_group_uuid = "100000000000000000000007"
    lproj_group_uuid = "100000000000000000000008"

    # File References
    ref_ios_app = "200000000000000000000001"
    ref_macos_app = "200000000000000000000002"
    ref_xcassets = "200000000000000000000003"
    ref_ios_plist = "200000000000000000000004"
    ref_ios_entitlements = "200000000000000000000005"
    ref_launch_storyboard = "200000000000000000000006"
    ref_macos_plist = "200000000000000000000007"
    ref_macos_entitlements = "200000000000000000000008"
    ref_build_cargo_sh = "200000000000000000000009"
    ref_generate_assets_sh = "20000000000000000000000A"
    ref_publish_xcode_sh = "20000000000000000000000B"

    # Build Files (for Resources / Frameworks)
    bf_ios_xcassets = "300000000000000000000001"
    bf_macos_xcassets = "300000000000000000000002"
    bf_ios_launch = "300000000000000000000003"

    # Native Targets
    target_ios_uuid = "400000000000000000000001"
    target_macos_uuid = "400000000000000000000002"

    # Build Phases - iOS
    bp_ios_cargo = "500000000000000000000001"
    bp_ios_res = "500000000000000000000002"
    bp_ios_fw = "500000000000000000000003"

    # Build Phases - macOS
    bp_macos_cargo = "500000000000000000000004"
    bp_macos_res = "500000000000000000000005"
    bp_macos_fw = "500000000000000000000006"

    # Configuration Lists & Configurations
    cfglist_proj = "600000000000000000000001"
    cfglist_ios = "600000000000000000000002"
    cfglist_macos = "600000000000000000000003"

    cfg_proj_debug = "700000000000000000000001"
    cfg_proj_release = "700000000000000000000002"
    cfg_ios_debug = "700000000000000000000003"
    cfg_ios_release = "700000000000000000000004"
    cfg_macos_debug = "700000000000000000000005"
    cfg_macos_release = "700000000000000000000006"

    pbxproj_content = f"""// !$*UTF8*$!
{{
	archiveVersion = 1;
	classes = {{
	}};
	objectVersion = 56;
	objects = {{

/* Begin PBXBuildFile section */
		{bf_ios_xcassets} /* Assets.xcassets in Resources */ = {{isa = PBXBuildFile; fileRef = {ref_xcassets} /* Assets.xcassets */; }};
		{bf_macos_xcassets} /* Assets.xcassets in Resources */ = {{isa = PBXBuildFile; fileRef = {ref_xcassets} /* Assets.xcassets */; }};
		{bf_ios_launch} /* LaunchScreen.storyboard in Resources */ = {{isa = PBXBuildFile; fileRef = {ref_launch_storyboard} /* LaunchScreen.storyboard */; }};
/* End PBXBuildFile section */

/* Begin PBXFileReference section */
		{ref_ios_app} /* Tabular.app */ = {{isa = PBXFileReference; explicitFileType = wrapper.application; includeInIndex = 0; path = Tabular.app; sourceTree = BUILT_PRODUCTS_DIR; }};
		{ref_macos_app} /* Tabular.app */ = {{isa = PBXFileReference; explicitFileType = wrapper.application; includeInIndex = 0; path = Tabular.app; sourceTree = BUILT_PRODUCTS_DIR; }};
		{ref_xcassets} /* Assets.xcassets */ = {{isa = PBXFileReference; lastKnownFileType = folder.assetcatalog; path = Assets.xcassets; sourceTree = "<group>"; }};
		{ref_ios_plist} /* Info.plist */ = {{isa = PBXFileReference; lastKnownFileType = text.plist.xml; path = Info.plist; sourceTree = "<group>"; }};
		{ref_ios_entitlements} /* Tabular-iOS.entitlements */ = {{isa = PBXFileReference; lastKnownFileType = text.plist.entitlements; path = "Tabular-iOS.entitlements"; sourceTree = "<group>"; }};
		{ref_launch_storyboard} /* LaunchScreen.storyboard */ = {{isa = PBXFileReference; fileEncoding = 4; lastKnownFileType = file.storyboard; path = LaunchScreen.storyboard; sourceTree = "<group>"; }};
		{ref_macos_plist} /* Info.plist */ = {{isa = PBXFileReference; lastKnownFileType = text.plist.xml; path = Info.plist; sourceTree = "<group>"; }};
		{ref_macos_entitlements} /* Tabular.entitlements */ = {{isa = PBXFileReference; lastKnownFileType = text.plist.entitlements; path = Tabular.entitlements; sourceTree = "<group>"; }};
		{ref_build_cargo_sh} /* build_cargo.sh */ = {{isa = PBXFileReference; fileEncoding = 4; lastKnownFileType = text.script.sh; path = build_cargo.sh; sourceTree = "<group>"; }};
		{ref_generate_assets_sh} /* generate_assets.sh */ = {{isa = PBXFileReference; fileEncoding = 4; lastKnownFileType = text.script.sh; path = generate_assets.sh; sourceTree = "<group>"; }};
		{ref_publish_xcode_sh} /* publish_xcode.sh */ = {{isa = PBXFileReference; fileEncoding = 4; lastKnownFileType = text.script.sh; path = publish_xcode.sh; sourceTree = "<group>"; }};
/* End PBXFileReference section */

/* Begin PBXFrameworksBuildPhase section */
		{bp_ios_fw} /* Frameworks */ = {{
			isa = PBXFrameworksBuildPhase;
			buildActionMask = 2147483647;
			files = (
			);
			runOnlyForDeploymentPostprocessing = 0;
		}};
		{bp_macos_fw} /* Frameworks */ = {{
			isa = PBXFrameworksBuildPhase;
			buildActionMask = 2147483647;
			files = (
			);
			runOnlyForDeploymentPostprocessing = 0;
		}};
/* End PBXFrameworksBuildPhase section */

/* Begin PBXGroup section */
		{main_group_uuid} = {{
			isa = PBXGroup;
			children = (
				{apple_group_uuid} /* apple */,
				{products_group_uuid} /* Products */,
			);
			sourceTree = "<group>";
		}};
		{products_group_uuid} /* Products */ = {{
			isa = PBXGroup;
			children = (
				{ref_ios_app} /* Tabular.app */,
				{ref_macos_app} /* Tabular.app */,
			);
			name = Products;
			sourceTree = "<group>";
		}};
		{apple_group_uuid} /* apple */ = {{
			isa = PBXGroup;
			children = (
				{ios_group_uuid} /* ios */,
				{macos_group_uuid} /* macos */,
				{ref_xcassets} /* Assets.xcassets */,
				{scripts_group_uuid} /* scripts */,
			);
			path = apple;
			sourceTree = "<group>";
		}};
		{ios_group_uuid} /* ios */ = {{
			isa = PBXGroup;
			children = (
				{lproj_group_uuid} /* Base.lproj */,
				{ref_ios_plist} /* Info.plist */,
				{ref_ios_entitlements} /* Tabular-iOS.entitlements */,
			);
			path = ios;
			sourceTree = "<group>";
		}};
		{lproj_group_uuid} /* Base.lproj */ = {{
			isa = PBXGroup;
			children = (
				{ref_launch_storyboard} /* LaunchScreen.storyboard */,
			);
			path = Base.lproj;
			sourceTree = "<group>";
		}};
		{macos_group_uuid} /* macos */ = {{
			isa = PBXGroup;
			children = (
				{ref_macos_plist} /* Info.plist */,
				{ref_macos_entitlements} /* Tabular.entitlements */,
			);
			path = macos;
			sourceTree = "<group>";
		}};
		{scripts_group_uuid} /* scripts */ = {{
			isa = PBXGroup;
			children = (
				{ref_build_cargo_sh} /* build_cargo.sh */,
				{ref_generate_assets_sh} /* generate_assets.sh */,
				{ref_publish_xcode_sh} /* publish_xcode.sh */,
			);
			path = scripts;
			sourceTree = "<group>";
		}};
/* End PBXGroup section */

/* Begin PBXNativeTarget section */
		{target_ios_uuid} /* Tabular-iOS */ = {{
			isa = PBXNativeTarget;
			buildConfigurationList = {cfglist_ios} /* Build configuration list for PBXNativeTarget "Tabular-iOS" */;
			buildPhases = (
				{bp_ios_cargo} /* Build Cargo (Rust) */,
				{bp_ios_res} /* Resources */,
				{bp_ios_fw} /* Frameworks */,
			);
			buildRules = (
			);
			dependencies = (
			);
			name = "Tabular-iOS";
			productName = Tabular;
			productReference = {ref_ios_app} /* Tabular.app */;
			productType = "com.apple.product-type.application";
		}};
		{target_macos_uuid} /* Tabular-macOS */ = {{
			isa = PBXNativeTarget;
			buildConfigurationList = {cfglist_macos} /* Build configuration list for PBXNativeTarget "Tabular-macOS" */;
			buildPhases = (
				{bp_macos_cargo} /* Build Cargo (Rust) */,
				{bp_macos_res} /* Resources */,
				{bp_macos_fw} /* Frameworks */,
			);
			buildRules = (
			);
			dependencies = (
			);
			name = "Tabular-macOS";
			productName = Tabular;
			productReference = {ref_macos_app} /* Tabular.app */;
			productType = "com.apple.product-type.application";
		}};
/* End PBXNativeTarget section */

/* Begin PBXProject section */
		{proj_uuid} /* Project object */ = {{
			isa = PBXProject;
			attributes = {{
				BuildIndependentTargetsInParallel = 1;
				LastUpgradeCheck = 1600;
				TargetAttributes = {{
					{target_ios_uuid} = {{
						CreatedOnToolsVersion = 16.0;
						DevelopmentTeam = YD4J5Z6A4G;
						ProvisioningStyle = Automatic;
					}};
					{target_macos_uuid} = {{
						CreatedOnToolsVersion = 16.0;
						DevelopmentTeam = YD4J5Z6A4G;
						ProvisioningStyle = Automatic;
					}};
				}};
			}};
			buildConfigurationList = {cfglist_proj} /* Build configuration list for PBXProject "Tabular" */;
			compatibilityVersion = "Xcode 14.0";
			developmentRegion = en;
			hasScannedForEncodings = 0;
			knownRegions = (
				en,
				Base,
			);
			mainGroup = {main_group_uuid};
			productRefGroup = {products_group_uuid} /* Products */;
			projectDirPath = "";
			projectRoot = "";
			targets = (
				{target_ios_uuid} /* Tabular-iOS */,
				{target_macos_uuid} /* Tabular-macOS */,
			);
		}};
/* End PBXProject section */

/* Begin PBXResourcesBuildPhase section */
		{bp_ios_res} /* Resources */ = {{
			isa = PBXResourcesBuildPhase;
			buildActionMask = 2147483647;
			files = (
				{bf_ios_xcassets} /* Assets.xcassets in Resources */,
				{bf_ios_launch} /* LaunchScreen.storyboard in Resources */,
			);
			runOnlyForDeploymentPostprocessing = 0;
		}};
		{bp_macos_res} /* Resources */ = {{
			isa = PBXResourcesBuildPhase;
			buildActionMask = 2147483647;
			files = (
				{bf_macos_xcassets} /* Assets.xcassets in Resources */,
			);
			runOnlyForDeploymentPostprocessing = 0;
		}};
/* End PBXResourcesBuildPhase section */

/* Begin PBXShellScriptBuildPhase section */
		{bp_ios_cargo} /* Build Cargo (Rust) */ = {{
			isa = PBXShellScriptBuildPhase;
			alwaysOutOfDate = 1;
			buildActionMask = 2147483647;
			files = (
			);
			inputFileListPaths = (
			);
			inputPaths = (
			);
			name = "Build Cargo (Rust)";
			outputFileListPaths = (
			);
			outputPaths = (
			);
			runOnlyForDeploymentPostprocessing = 0;
			shellPath = /bin/sh;
			shellScript = "\\"${{PROJECT_DIR}}/apple/scripts/build_cargo.sh\\"\\n";
		}};
		{bp_macos_cargo} /* Build Cargo (Rust) */ = {{
			isa = PBXShellScriptBuildPhase;
			alwaysOutOfDate = 1;
			buildActionMask = 2147483647;
			files = (
			);
			inputFileListPaths = (
			);
			inputPaths = (
			);
			name = "Build Cargo (Rust)";
			outputFileListPaths = (
			);
			outputPaths = (
			);
			runOnlyForDeploymentPostprocessing = 0;
			shellPath = /bin/sh;
			shellScript = "\\"${{PROJECT_DIR}}/apple/scripts/build_cargo.sh\\"\\n";
		}};
/* End PBXShellScriptBuildPhase section */

/* Begin XCBuildConfiguration section */
		{cfg_proj_debug} /* Debug */ = {{
			isa = XCBuildConfiguration;
			buildSettings = {{
				ALWAYS_SEARCH_USER_PATHS = NO;
				CLANG_ANALYZER_NONNULL = YES;
				COPY_PHASE_STRIP = NO;
				DEBUG_INFORMATION_FORMAT = "dwarf-with-dsym";
				ENABLE_BITCODE = NO;
				ENABLE_STRICT_OBJC_MSGSEND = YES;
				ENABLE_TESTABILITY = YES;
				GCC_DYNAMIC_NO_PIC = NO;
				GCC_OPTIMIZATION_LEVEL = 0;
				GCC_PREPROCESSOR_DEFINITIONS = (
					"DEBUG=1",
					"$(inherited)",
				);
				MTL_ENABLE_DEBUG_INFO = INCLUDE_SOURCE;
				MTL_FAST_MATH = YES;
				ONLY_ACTIVE_ARCH = YES;
			}};
			name = Debug;
		}};
		{cfg_proj_release} /* Release */ = {{
			isa = XCBuildConfiguration;
			buildSettings = {{
				ALWAYS_SEARCH_USER_PATHS = NO;
				CLANG_ANALYZER_NONNULL = YES;
				COPY_PHASE_STRIP = NO;
				DEBUG_INFORMATION_FORMAT = "dwarf-with-dsym";
				ENABLE_BITCODE = NO;
				ENABLE_NS_ASSERTIONS = NO;
				ENABLE_STRICT_OBJC_MSGSEND = YES;
				GCC_NO_COMMON_BLOCKS = YES;
				MTL_ENABLE_DEBUG_INFO = NO;
				MTL_FAST_MATH = YES;
			}};
			name = Release;
		}};
		{cfg_ios_debug} /* Debug */ = {{
			isa = XCBuildConfiguration;
			buildSettings = {{
				ASSETCATALOG_COMPILER_APPICON_NAME = AppIcon;
				ASSETCATALOG_COMPILER_GLOBAL_ACCENT_COLOR_NAME = AccentColor;
				CODE_SIGN_ENTITLEMENTS = "apple/ios/Tabular-iOS.entitlements";
				CODE_SIGN_STYLE = Automatic;
				CURRENT_PROJECT_VERSION = 1;
				DEVELOPMENT_TEAM = YD4J5Z6A4G;
				GENERATE_INFOPLIST_FILE = NO;
				INFOPLIST_FILE = apple/ios/Info.plist;
				IPHONEOS_DEPLOYMENT_TARGET = 16.0;
				LD_RUNPATH_SEARCH_PATHS = (
					"$(inherited)",
					"@executable_path/Frameworks",
				);
				MARKETING_VERSION = {version};
				PRODUCT_BUNDLE_IDENTIFIER = id.tabular.database;
				PRODUCT_NAME = Tabular;
				SDKROOT = iphoneos;
				SUPPORTED_PLATFORMS = "iphoneos iphonesimulator";
				SUPPORTS_MACCATALYST = NO;
				SUPPORTS_MAC_DESIGNED_FOR_IPHONE_IPAD = NO;
				TARGETED_DEVICE_FAMILY = "1,2";
			}};
			name = Debug;
		}};
		{cfg_ios_release} /* Release */ = {{
			isa = XCBuildConfiguration;
			buildSettings = {{
				ASSETCATALOG_COMPILER_APPICON_NAME = AppIcon;
				ASSETCATALOG_COMPILER_GLOBAL_ACCENT_COLOR_NAME = AccentColor;
				CODE_SIGN_ENTITLEMENTS = "apple/ios/Tabular-iOS.entitlements";
				CODE_SIGN_STYLE = Automatic;
				CURRENT_PROJECT_VERSION = 1;
				DEVELOPMENT_TEAM = YD4J5Z6A4G;
				GENERATE_INFOPLIST_FILE = NO;
				INFOPLIST_FILE = apple/ios/Info.plist;
				IPHONEOS_DEPLOYMENT_TARGET = 16.0;
				LD_RUNPATH_SEARCH_PATHS = (
					"$(inherited)",
					"@executable_path/Frameworks",
				);
				MARKETING_VERSION = {version};
				PRODUCT_BUNDLE_IDENTIFIER = id.tabular.database;
				PRODUCT_NAME = Tabular;
				SDKROOT = iphoneos;
				SUPPORTED_PLATFORMS = "iphoneos iphonesimulator";
				SUPPORTS_MACCATALYST = NO;
				SUPPORTS_MAC_DESIGNED_FOR_IPHONE_IPAD = NO;
				TARGETED_DEVICE_FAMILY = "1,2";
				VALIDATE_PRODUCT = YES;
			}};
			name = Release;
		}};
		{cfg_macos_debug} /* Debug */ = {{
			isa = XCBuildConfiguration;
			buildSettings = {{
				ASSETCATALOG_COMPILER_APPICON_NAME = AppIcon;
				ASSETCATALOG_COMPILER_GLOBAL_ACCENT_COLOR_NAME = AccentColor;
				CODE_SIGN_ENTITLEMENTS = apple/macos/Tabular.entitlements;
				CODE_SIGN_STYLE = Automatic;
				COMBINE_HIDPI_IMAGES = YES;
				CURRENT_PROJECT_VERSION = 1;
				DEVELOPMENT_TEAM = YD4J5Z6A4G;
				ENABLE_HARDENED_RUNTIME = YES;
				GENERATE_INFOPLIST_FILE = NO;
				INFOPLIST_FILE = apple/macos/Info.plist;
				LD_RUNPATH_SEARCH_PATHS = (
					"$(inherited)",
					"@executable_path/../Frameworks",
				);
				MACOSX_DEPLOYMENT_TARGET = 13.0;
				MARKETING_VERSION = {version};
				PRODUCT_BUNDLE_IDENTIFIER = id.tabular.database;
				PRODUCT_NAME = Tabular;
				SDKROOT = macosx;
			}};
			name = Debug;
		}};
		{cfg_macos_release} /* Release */ = {{
			isa = XCBuildConfiguration;
			buildSettings = {{
				ASSETCATALOG_COMPILER_APPICON_NAME = AppIcon;
				ASSETCATALOG_COMPILER_GLOBAL_ACCENT_COLOR_NAME = AccentColor;
				CODE_SIGN_ENTITLEMENTS = apple/macos/Tabular.entitlements;
				CODE_SIGN_STYLE = Automatic;
				COMBINE_HIDPI_IMAGES = YES;
				CURRENT_PROJECT_VERSION = 1;
				DEVELOPMENT_TEAM = YD4J5Z6A4G;
				ENABLE_HARDENED_RUNTIME = YES;
				GENERATE_INFOPLIST_FILE = NO;
				INFOPLIST_FILE = apple/macos/Info.plist;
				LD_RUNPATH_SEARCH_PATHS = (
					"$(inherited)",
					"@executable_path/../Frameworks",
				);
				MACOSX_DEPLOYMENT_TARGET = 13.0;
				MARKETING_VERSION = {version};
				PRODUCT_BUNDLE_IDENTIFIER = id.tabular.database;
				PRODUCT_NAME = Tabular;
				SDKROOT = macosx;
			}};
			name = Release;
		}};
/* End XCBuildConfiguration section */

/* Begin XCConfigurationList section */
		{cfglist_proj} /* Build configuration list for PBXProject "Tabular" */ = {{
			isa = XCConfigurationList;
			buildConfigurations = (
				{cfg_proj_debug} /* Debug */,
				{cfg_proj_release} /* Release */,
			);
			defaultConfigurationIsVisible = 0;
			defaultConfigurationName = Release;
		}};
		{cfglist_ios} /* Build configuration list for PBXNativeTarget "Tabular-iOS" */ = {{
			isa = XCConfigurationList;
			buildConfigurations = (
				{cfg_ios_debug} /* Debug */,
				{cfg_ios_release} /* Release */,
			);
			defaultConfigurationIsVisible = 0;
			defaultConfigurationName = Release;
		}};
		{cfglist_macos} /* Build configuration list for PBXNativeTarget "Tabular-macOS" */ = {{
			isa = XCConfigurationList;
			buildConfigurations = (
				{cfg_macos_debug} /* Debug */,
				{cfg_macos_release} /* Release */,
			);
			defaultConfigurationIsVisible = 0;
			defaultConfigurationName = Release;
		}};
/* End XCConfigurationList section */

	}};
	rootObject = {proj_uuid} /* Project object */;
}}
"""

    pbxproj_path = os.path.join(xcode_dir, "project.pbxproj")
    with open(pbxproj_path, "w", encoding="utf-8") as f:
        f.write(pbxproj_content)
    print(f"[SUCCESS] Wrote {pbxproj_path}")

    # Shared Scheme for Tabular-iOS
    scheme_ios_content = f"""<?xml version="1.0" encoding="UTF-8"?>
<Scheme
   LastUpgradeVersion = "1600"
   version = "1.7">
   <BuildAction
      parallelizeBuildables = "YES"
      buildImplicitDependencies = "YES">
      <BuildActionEntries>
         <BuildActionEntry
            buildForTesting = "YES"
            buildForRunning = "YES"
            buildForProfiling = "YES"
            buildForArchiving = "YES"
            buildForAnalyzing = "YES">
            <BuildableReference
               BuildableIdentifier = "primary"
               BlueprintIdentifier = "{target_ios_uuid}"
               BuildableName = "Tabular.app"
               BlueprintName = "Tabular-iOS"
               ReferencedContainer = "container:Tabular.xcodeproj">
            </BuildableReference>
         </BuildActionEntry>
      </BuildActionEntries>
   </BuildAction>
   <TestAction
      buildConfiguration = "Debug"
      selectedDebuggerIdentifier = "Xcode.DebuggerFoundation.Debugger.LLDB"
      selectedLauncherIdentifier = "Xcode.DebuggerFoundation.Launcher.LLDB"
      shouldUseLaunchSchemeArgsEnv = "YES">
      <Testables>
      </Testables>
   </TestAction>
   <LaunchAction
      buildConfiguration = "Release"
      selectedDebuggerIdentifier = ""
      selectedLauncherIdentifier = "Xcode.IDEFoundation.Launcher.PosixSpawn"
      launchStyle = "0"
      useCustomWorkingDirectory = "NO"
      ignoresPersistentStateOnLaunch = "NO"
      debugDocumentVersioning = "YES"
      debugServiceExtension = "internal"
      allowLocationSimulation = "YES">
      <BuildableProductRunnable
         runnableDebuggingMode = "0">
         <BuildableReference
            BuildableIdentifier = "primary"
            BlueprintIdentifier = "{target_ios_uuid}"
            BuildableName = "Tabular.app"
            BlueprintName = "Tabular-iOS"
            ReferencedContainer = "container:Tabular.xcodeproj">
         </BuildableReference>
      </BuildableProductRunnable>
   </LaunchAction>
   <ProfileAction
      buildConfiguration = "Release"
      shouldUseLaunchSchemeArgsEnv = "YES"
      savedToolIdentifier = ""
      useCustomWorkingDirectory = "NO"
      debugDocumentVersioning = "YES">
      <BuildableProductRunnable
         runnableDebuggingMode = "0">
         <BuildableReference
            BuildableIdentifier = "primary"
            BlueprintIdentifier = "{target_ios_uuid}"
            BuildableName = "Tabular.app"
            BlueprintName = "Tabular-iOS"
            ReferencedContainer = "container:Tabular.xcodeproj">
         </BuildableReference>
      </BuildableProductRunnable>
   </ProfileAction>
   <AnalyzeAction
      buildConfiguration = "Debug">
   </AnalyzeAction>
   <ArchiveAction
      buildConfiguration = "Release"
      revealArchiveInOrganizer = "YES">
   </ArchiveAction>
</Scheme>
"""
    scheme_ios_path = os.path.join(schemes_dir, "Tabular-iOS.xcscheme")
    with open(scheme_ios_path, "w", encoding="utf-8") as f:
        f.write(scheme_ios_content)
    print(f"[SUCCESS] Wrote {scheme_ios_path}")

    # Shared Scheme for Tabular-macOS
    scheme_macos_content = f"""<?xml version="1.0" encoding="UTF-8"?>
<Scheme
   LastUpgradeVersion = "1600"
   version = "1.7">
   <BuildAction
      parallelizeBuildables = "YES"
      buildImplicitDependencies = "YES">
      <BuildActionEntries>
         <BuildActionEntry
            buildForTesting = "YES"
            buildForRunning = "YES"
            buildForProfiling = "YES"
            buildForArchiving = "YES"
            buildForAnalyzing = "YES">
            <BuildableReference
               BuildableIdentifier = "primary"
               BlueprintIdentifier = "{target_macos_uuid}"
               BuildableName = "Tabular.app"
               BlueprintName = "Tabular-macOS"
               ReferencedContainer = "container:Tabular.xcodeproj">
            </BuildableReference>
         </BuildActionEntry>
      </BuildActionEntries>
   </BuildAction>
   <TestAction
      buildConfiguration = "Debug"
      selectedDebuggerIdentifier = "Xcode.DebuggerFoundation.Debugger.LLDB"
      selectedLauncherIdentifier = "Xcode.DebuggerFoundation.Launcher.LLDB"
      shouldUseLaunchSchemeArgsEnv = "YES">
      <Testables>
      </Testables>
   </TestAction>
   <LaunchAction
      buildConfiguration = "Release"
      selectedDebuggerIdentifier = ""
      selectedLauncherIdentifier = "Xcode.IDEFoundation.Launcher.PosixSpawn"
      launchStyle = "0"
      useCustomWorkingDirectory = "NO"
      ignoresPersistentStateOnLaunch = "NO"
      debugDocumentVersioning = "YES"
      debugServiceExtension = "internal"
      allowLocationSimulation = "YES">
      <BuildableProductRunnable
         runnableDebuggingMode = "0">
         <BuildableReference
            BuildableIdentifier = "primary"
            BlueprintIdentifier = "{target_macos_uuid}"
            BuildableName = "Tabular.app"
            BlueprintName = "Tabular-macOS"
            ReferencedContainer = "container:Tabular.xcodeproj">
         </BuildableReference>
      </BuildableProductRunnable>
   </LaunchAction>
   <ProfileAction
      buildConfiguration = "Release"
      shouldUseLaunchSchemeArgsEnv = "YES"
      savedToolIdentifier = ""
      useCustomWorkingDirectory = "NO"
      debugDocumentVersioning = "YES">
      <BuildableProductRunnable
         runnableDebuggingMode = "0">
         <BuildableReference
            BuildableIdentifier = "primary"
            BlueprintIdentifier = "{target_macos_uuid}"
            BuildableName = "Tabular.app"
            BlueprintName = "Tabular-macOS"
            ReferencedContainer = "container:Tabular.xcodeproj">
         </BuildableReference>
      </BuildableProductRunnable>
   </ProfileAction>
   <AnalyzeAction
      buildConfiguration = "Debug">
   </AnalyzeAction>
   <ArchiveAction
      buildConfiguration = "Release"
      revealArchiveInOrganizer = "YES">
   </ArchiveAction>
</Scheme>
"""
    scheme_macos_path = os.path.join(schemes_dir, "Tabular-macOS.xcscheme")
    with open(scheme_macos_path, "w", encoding="utf-8") as f:
        f.write(scheme_macos_content)
    print(f"[SUCCESS] Wrote {scheme_macos_path}")

if __name__ == "__main__":
    main()
