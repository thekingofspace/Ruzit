--[[
This next task is gonna be a very large one,

we are depreciating build.toml

]]
export type shard = {
    
}

export type package = {
    id:string, -- "@Game" or like "@DlcID"
    EncryptionToken:string, -- used by some encryption methods all tokens are built into the EXE
    CompressScripts:boolean,
    ConvertToByte:boolean, -- convert code to byte code
    CreateShard:(self:package, ID:number) -> shard, -- needed for what assets get sent to what managed file, each shard needs a ID number, numbers must contain numbers before it so you can define 3,1,2 and it works but if you do 4, 1,2 then it rhwos a error
    ParseAssets:(self:package, Callback:(AssetPath:string, Size:number) -> (shard, boolean, string)) -> () -- shard to save it to second bool is wether or not the file should be compressed last var is a encryption method to use, example path @DLCID/Test/This.fbx or @Game/Images/Logo.png
}

export type build = {
    name:string,
    version:string,
    creator:string,
    ExeName:string,
    Icon:string, -- path to icon
    Windowed:boolean,
    FileType: "Relative" | "Global",
    SteamID:number,
    include:{string},
    GetPackages:() -> {package},
}

local build = nil::build

for _, item in ipairs(build.GetPackages()) do
    item.CompressScripts = false
    item.ConvertToByte = true

    local FontShard = item:CreateShard(1)

    item:ParseAssets(function(asset, Size)
        return FontShard, true
    end)
end

export type ManifestPackage = {
    Free:(self:Manifest) -> () --[[
        When freeing a manifest it does not free any already loaded assets via GetAsset or other things, this just unloads the asset file from memory, however script files remain cached.
    ]]
}

export type Manifest = {
    LoadManaged:(id:string) -> ManifestPackage -- throw error if managed is already loaded
}

local manifest = nil::Manifest

local package = manifest.LoadManaged("@Common") -- loads scripts so they can be accsesed via require and load's assets, throw error if already loaded.

-- code here

package:Free()

--[[
    WHOLE CONCEPT

    So in this system we change how things are loaded, manifest files and its shards are not auto loaded, so require and assets can not find, excluding @Game that is, which is always loaded.

    Inside build.luau it contains the key word build with the provided types.

    when loading a manged you gain access to its things, upon loading it doesnt decrypt or decompress assets until you request them via GetAsset or any other asset method. to keep memory low.

    this is a very custom system for how to assembly your games.

    if the primitve build.toml is present use that if no build.luau is found. build.luau is the new prefered method

    in gamke you need to init managed files using manifest before you can use them for any of the standard libraries like Managed, assets etc etc.

    when in the test enviroment it will skip all of this, basically how it will work is if you havent loaded it via manfiest it will throw a warning in the logs but won't cause errors for debuggin reasons.
]]