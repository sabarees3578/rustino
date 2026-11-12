#include <Wire.h>
#include <Adafruit_GFX.h>
#include <Adafruit_SSD1306.h>

#define SCREEN_WIDTH 128
#define SCREEN_HEIGHT 64
#define OLED_RESET    -1
#define SCREEN_ADDRESS 0x3C

// I2C Pins for OLED
#define I2C_SDA 8
#define I2C_SCL 9

// Touch Sensor Pins
#define TOUCH_GND_PIN 3   // GPIO 3 acting as GND for touch sensor
#define TOUCH_SIG_PIN 21  // GPIO 21 reading touch signal

Adafruit_SSD1306 display(SCREEN_WIDTH, SCREEN_HEIGHT, &Wire, OLED_RESET);

// Bot Mood States
int currentMood = 0; // 0: Idle, 1: Happy, 2: Love, 3: Angry, 4: Surprised
bool lastTouchState = LOW;
unsigned long touchStartTime = 0;
unsigned long lastBlinkTime = 0;
unsigned long lastLookTime = 0;
int eyeOffsetX = 0;
int eyeOffsetY = 0;

// 16x16 Heart Bitmap for Love Mode
const unsigned char heartBMP[] PROGMEM = {
  0x00, 0x00, 0x1c, 0x38, 0x3e, 0x7c, 0x7f, 0xfe, 
  0x7f, 0xfe, 0x7f, 0xfe, 0x3f, 0xfc, 0x1f, 0xf8, 
  0x0f, 0xf0, 0x07, 0xe0, 0x03, 0xc0, 0x01, 0x80, 
  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00
};

void setup() {
  Serial.begin(115200);

  // Set GPIO 3 as LOW to provide Ground for the Touch Sensor
  pinMode(TOUCH_GND_PIN, OUTPUT);
  digitalWrite(TOUCH_GND_PIN, LOW);

  // Set GPIO 21 as Input for Touch Sensor Signal
  pinMode(TOUCH_SIG_PIN, INPUT);

  // Start I2C and OLED
  Wire.begin(I2C_SDA, I2C_SCL);
  if (!display.begin(SSD1306_SWITCHCAPVCC, SCREEN_ADDRESS)) {
    Serial.println(F("SSD1306 allocation failed"));
    for (;;);
  }

  display.clearDisplay();
  drawBotIdle(0, 0, false);
}

void loop() {
  bool isTouched = digitalRead(TOUCH_SIG_PIN);
  unsigned long now = millis();

  // Detect new touch press
  if (isTouched == HIGH && lastTouchState == LOW) {
    touchStartTime = now;
    currentMood = (currentMood + 1) % 5; // Cycle through 5 moods
  }

  // If holding touch for > 600ms ("Petting" the bot)
  if (isTouched == HIGH && (now - touchStartTime > 600)) {
    drawBotPetted();
    lastTouchState = isTouched;
    delay(50);
    return;
  }

  lastTouchState = isTouched;

  // Render current mood
  if (currentMood == 0) {
    // Autonomous glancing around every 2.5 seconds
    if (now - lastLookTime > 2500) {
      int dir = random(0, 5);
      if (dir == 0) { eyeOffsetX = 0; eyeOffsetY = 0; }
      else if (dir == 1) { eyeOffsetX = -8; eyeOffsetY = 0; }
      else if (dir == 2) { eyeOffsetX = 8; eyeOffsetY = 0; }
      else if (dir == 3) { eyeOffsetX = 0; eyeOffsetY = -4; }
      else { eyeOffsetX = 0; eyeOffsetY = 4; }
      lastLookTime = now;
    }

    // Autonomous blinking every 3.5 seconds
    if (now - lastBlinkTime > 3500) {
      drawBotIdle(eyeOffsetX, eyeOffsetY, true); // Blink closed
      delay(120);
      lastBlinkTime = now;
    } else {
      drawBotIdle(eyeOffsetX, eyeOffsetY, false); // Eyes open
    }
  } 
  else if (currentMood == 1) {
    drawBotHappy();
  } 
  else if (currentMood == 2) {
    drawBotLove();
  } 
  else if (currentMood == 3) {
    drawBotAngry();
  } 
  else if (currentMood == 4) {
    drawBotSurprised();
  }

  delay(40);
}

// 0. IDLE BOT EYES (Rounded rectangles like EMO / Cozmo)
void drawBotIdle(int offX, int offY, bool blink) {
  display.clearDisplay();
  if (blink) {
    // Closed flat eyes
    display.fillRoundRect(22 + offX, 30 + offY, 32, 6, 3, SSD1306_WHITE);
    display.fillRoundRect(74 + offX, 30 + offY, 32, 6, 3, SSD1306_WHITE);
  } else {
    // Open rounded bot eyes
    display.fillRoundRect(22 + offX, 16 + offY, 32, 32, 8, SSD1306_WHITE);
    display.fillRoundRect(74 + offX, 16 + offY, 32, 32, 8, SSD1306_WHITE);
  }
  display.display();
}

// 1. HAPPY BOT EYES (^ ^)
void drawBotHappy() {
  display.clearDisplay();
  // Left Happy Eye (Thick Arc using overlapping circles)
  display.fillRoundRect(22, 18, 32, 28, 12, SSD1306_WHITE);
  display.fillRoundRect(22, 28, 32, 24, 10, SSD1306_BLACK);

  // Right Happy Eye
  display.fillRoundRect(74, 18, 32, 28, 12, SSD1306_WHITE);
  display.fillRoundRect(74, 28, 32, 24, 10, SSD1306_BLACK);

  // Cute tiny bot mouth
  display.drawLine(58, 52, 64, 56, SSD1306_WHITE);
  display.drawLine(64, 56, 70, 52, SSD1306_WHITE);
  display.display();
}

// 2. LOVE BOT MODE
void drawBotLove() {
  display.clearDisplay();
  // Eyes with floating hearts
  display.fillRoundRect(22, 20, 32, 28, 10, SSD1306_WHITE);
  display.fillRoundRect(22, 30, 32, 22, 8, SSD1306_BLACK);

  display.fillRoundRect(74, 20, 32, 28, 10, SSD1306_WHITE);
  display.fillRoundRect(74, 30, 32, 22, 8, SSD1306_BLACK);

  // Heart icon in the top center
  display.drawBitmap(56, 2, heartBMP, 16, 16, SSD1306_WHITE);
  display.display();
}

// 3. ANGRY BOT EYES
void drawBotAngry() {
  display.clearDisplay();
  // Base eyes
  display.fillRoundRect(22, 18, 32, 28, 6, SSD1306_WHITE);
  display.fillRoundRect(74, 18, 32, 28, 6, SSD1306_WHITE);

  // Slanted black triangles on top to create angry brow slant
  display.fillTriangle(20, 16, 56, 16, 56, 32, SSD1306_BLACK);
  display.fillTriangle(72, 16, 108, 16, 72, 32, SSD1306_BLACK);
  display.display();
}

// 4. SURPRISED / SHOCKED BOT EYES
void drawBotSurprised() {
  display.clearDisplay();
  // Hollow glowing ring eyes
  display.drawCircle(38, 30, 15, SSD1306_WHITE);
  display.drawCircle(38, 30, 14, SSD1306_WHITE);
  display.fillCircle(38, 30, 4, SSD1306_WHITE);

  display.drawCircle(90, 30, 15, SSD1306_WHITE);
  display.drawCircle(90, 30, 14, SSD1306_WHITE);
  display.fillCircle(90, 30, 4, SSD1306_WHITE);

  // Small 'o' mouth
  display.drawCircle(64, 52, 4, SSD1306_WHITE);
  display.display();
}

// SPECIAL: PETTING ANIMATION (When holding finger on touch sensor)
void drawBotPetted() {
  display.clearDisplay();
  // Squinting happy eyes
  display.fillRoundRect(22, 22, 32, 24, 10, SSD1306_WHITE);
  display.fillRoundRect(22, 30, 32, 20, 8, SSD1306_BLACK);

  display.fillRoundRect(74, 22, 32, 24, 10, SSD1306_WHITE);
  display.fillRoundRect(74, 30, 32, 20, 8, SSD1306_BLACK);

  // Blush lines under eyes
  display.drawLine(16, 46, 20, 42, SSD1306_WHITE);
  display.drawLine(22, 46, 26, 42, SSD1306_WHITE);
  display.drawLine(102, 46, 106, 42, SSD1306_WHITE);
  display.drawLine(108, 46, 112, 42, SSD1306_WHITE);

  // Floating heart
  display.drawBitmap(56, 4, heartBMP, 16, 16, SSD1306_WHITE);
  display.display();
}