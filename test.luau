local VR = import("VirtualReality")
local primitives = import("Primitives")

if VR.HasVrFlag and VR.IsVrPresent then
    VR.LinkCamera()

    VR.HeadMoved:Connect(function()
        print(VR:HeadToWorldSpace())
    end)

    VR.BodyCFrame = primitives.CFrame.new(vector.create(0,1,0))

    local conns = VR.GetControllers() -- {Left, Right}

    local LeftController = conns[1]

    LeftController.Moved:Connect(function()
        print(LeftController:ToWorldSpace())
    end)
end
