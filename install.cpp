#include <openvr/openvr.h>
#include <iostream>
#include <filesystem>

namespace fs = std::filesystem;

static constexpr const char *APP_KEY = "org.mon-ouie.mpris-openvr-overlay";

int main(int argc, char *argv[]) {
  vr::EVRInitError error = vr::VRInitError_None;
  vr::VR_Init(&error, vr::VRApplication_Background);

  if (error != vr::VRInitError_None) {
    char buf[1024];
    snprintf(buf, sizeof(buf), "Unable to init VR runtime: %s",
             vr::VR_GetVRInitErrorAsEnglishDescription(error));
    std::cerr << buf << "\n";
    return 1;
  }

  if (!vr::VRApplications()) {
    std::cerr << "Failed to access VR applications!" << "\n";
    vr::VR_Shutdown();
    return 1;
  }

  if (!vr::VRApplications()->IsApplicationInstalled(APP_KEY)) {
    std::string absolute_path = fs::absolute("manifest.vrmanifest");
    vr::EVRApplicationError err =
      vr::VRApplications()->AddApplicationManifest(absolute_path.c_str());

    if (err != vr::VRApplicationError_None) {
      std::cerr << "Failed to install application: " << err << "\n";
      vr::VR_Shutdown();
      return 1;
    }

    std::cout << "Application installed successfully!\n";
  }
  else
    std::cout << "Application already installed" << "\n";

  vr::EVRApplicationError err =
    vr::VRApplications()->LaunchDashboardOverlay(APP_KEY);
  if (err != vr::VRApplicationError_None) {
    std::cerr << "Failed launching overlay: " << err << "\n";

    vr::VR_Shutdown();
    return 0;
  }

  vr::VR_Shutdown();
  return 0;
}
